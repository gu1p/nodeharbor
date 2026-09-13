#include "socket_adapter.h"
#include <slirp/libslirp.h>
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define OUTPUT_CAPACITY (1024 * 1024)
typedef struct Timer {
    SlirpTimerCb callback;
    void *opaque;
    int64_t deadline;
    struct Timer *next;
} Timer;
typedef struct {
    int fd, stop;
    Slirp *slirp;
    struct pollfd polls[256];
    int poll_count;
    Timer *timers;
    uint8_t input[65540];
    size_t received, expected;
    uint8_t output[OUTPUT_CAPACITY];
    size_t output_read, output_size;
} Loop;

static int64_t clock_ns(void *opaque) {
    (void)opaque;
    struct timespec time;
    clock_gettime(CLOCK_MONOTONIC, &time);
    return (int64_t)time.tv_sec * 1000000000 + time.tv_nsec;
}
static void noop(void *opaque) { (void)opaque; }
static void socket_noop(slirp_os_socket fd, void *opaque) { (void)fd; (void)opaque; }
static void guest_error(const char *message, void *opaque) { (void)message; (void)opaque; }
static void enqueue(Loop *loop, const uint8_t *data, size_t size) {
    size_t write_at = (loop->output_read + loop->output_size) % OUTPUT_CAPACITY;
    size_t first = size < OUTPUT_CAPACITY - write_at ? size : OUTPUT_CAPACITY - write_at;
    memcpy(loop->output + write_at, data, first);
    memcpy(loop->output, data + first, size - first);
    loop->output_size += size;
}
static ssize_t send_packet(const void *packet, size_t size, void *opaque) {
    Loop *loop = opaque;
    if (size > 65536) return -1;
    if (size + 4 > OUTPUT_CAPACITY - loop->output_size) return (ssize_t)size;
    uint32_t length = htonl((uint32_t)size);
    enqueue(loop, (const uint8_t *)&length, 4);
    enqueue(loop, packet, size);
    return (ssize_t)size;
}
static void flush_output(Loop *loop) {
    while (loop->output_size) {
        size_t count = loop->output_size < OUTPUT_CAPACITY - loop->output_read ? loop->output_size : OUTPUT_CAPACITY - loop->output_read;
        ssize_t sent = send(loop->fd, loop->output + loop->output_read, count, MSG_DONTWAIT | MSG_NOSIGNAL);
        if (sent < 0 && errno == EINTR) continue;
        if (sent < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return;
        if (sent <= 0) { loop->stop = 1; return; }
        loop->output_read = (loop->output_read + (size_t)sent) % OUTPUT_CAPACITY;
        loop->output_size -= (size_t)sent;
    }
}
static void read_packets(Loop *loop) {
    for (int packet_count = 0; packet_count < 64; packet_count++) {
        size_t wanted = loop->expected ? loop->expected : 4;
        ssize_t count = recv(loop->fd, loop->input + loop->received, wanted - loop->received, MSG_DONTWAIT);
        if (count < 0 && errno == EINTR) continue;
        if (count < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return;
        if (count <= 0) { loop->stop = 1; return; }
        loop->received += (size_t)count;
        if (!loop->expected && loop->received == 4) {
            uint32_t length;
            memcpy(&length, loop->input, 4);
            length = ntohl(length);
            if (length < 14 || length > 65536) { loop->stop = 1; return; }
            loop->expected = (size_t)length + 4;
        }
        if (loop->expected && loop->received == loop->expected) {
            slirp_input(loop->slirp, loop->input + 4, (int)(loop->expected - 4));
            loop->expected = loop->received = 0;
        }
    }
}
static void *timer_new(SlirpTimerCb callback, void *callback_opaque, void *opaque) {
    Loop *loop = opaque;
    Timer *timer = calloc(1, sizeof(*timer));
    if (!timer) { loop->stop = 1; return NULL; }
    *timer = (Timer){.callback = callback, .opaque = callback_opaque, .deadline = -1, .next = loop->timers};
    loop->timers = timer;
    return timer;
}
static void timer_free(void *timer, void *opaque) {
    Loop *loop = opaque;
    Timer **current = &loop->timers;
    while (*current && *current != timer) current = &(*current)->next;
    if (*current) { Timer *removed = *current; *current = removed->next; free(removed); }
}
static void timer_mod(void *timer, int64_t deadline, void *opaque) {
    (void)opaque;
    if (timer) ((Timer *)timer)->deadline = deadline;
}
static int add_poll(slirp_os_socket fd, int events, void *opaque) {
    Loop *loop = opaque;
    if (loop->poll_count >= 256) { loop->stop = 1; return -1; }
    int index = loop->poll_count++;
    short flags = 0;
    if (events & SLIRP_POLL_IN) flags |= POLLIN;
    if (events & SLIRP_POLL_OUT) flags |= POLLOUT;
    if (events & SLIRP_POLL_PRI) flags |= POLLPRI;
    loop->polls[index] = (struct pollfd){.fd = fd, .events = flags};
    return index;
}
static int get_revents(int index, void *opaque) {
    Loop *loop = opaque;
    if (index < 0 || index >= loop->poll_count) return 0;
    short events = loop->polls[index].revents;
    return ((events & POLLIN) ? SLIRP_POLL_IN : 0) | ((events & POLLOUT) ? SLIRP_POLL_OUT : 0) |
        ((events & POLLPRI) ? SLIRP_POLL_PRI : 0) | ((events & POLLERR) ? SLIRP_POLL_ERR : 0) |
        ((events & POLLHUP) ? SLIRP_POLL_HUP : 0);
}

JNIEXPORT void JNICALL
Java_io_github_gu1p_nodeharbor_NativeNetwork_run(JNIEnv *env, jobject self, jint fd, jbyteArray dns, jobject bridge) {
    (void)self;
    if (fd < 3 || !dns || (*env)->GetArrayLength(env, dns) != 4 || !bridge) goto invalid;
    if (nh_broker_begin(env, bridge)) goto invalid;
    Loop *loop = calloc(1, sizeof(*loop));
    if (!loop) { nh_broker_end(); goto invalid; }
    loop->fd = fd;
    SlirpConfig config = {.version = 6, .in_enabled = true, .in6_enabled = false,
        .if_mtu = 1500, .if_mru = 1500, .disable_host_loopback = true, .disable_dns = true};
    inet_pton(AF_INET, "10.0.2.0", &config.vnetwork);
    inet_pton(AF_INET, "255.255.255.0", &config.vnetmask);
    inet_pton(AF_INET, "10.0.2.2", &config.vhost);
    inet_pton(AF_INET, "10.0.2.15", &config.vdhcp_start);
    (*env)->GetByteArrayRegion(env, dns, 0, 4, (jbyte *)&config.vnameserver);
    SlirpCb callbacks = {.send_packet = send_packet, .guest_error = guest_error, .clock_get_ns = clock_ns,
        .timer_new = timer_new, .timer_free = timer_free, .timer_mod = timer_mod, .notify = noop,
        .register_poll_socket = socket_noop, .unregister_poll_socket = socket_noop};
    loop->slirp = slirp_new(&config, &callbacks, loop);
    if (!loop->slirp) loop->stop = 1;
    while (!loop->stop) {
        uint32_t timeout = 100;
        loop->poll_count = 1;
        loop->polls[0] = (struct pollfd){.fd = fd, .events = POLLIN | (loop->output_size ? POLLOUT : 0)};
        slirp_pollfds_fill_socket(loop->slirp, &timeout, add_poll, loop);
        int64_t now = clock_ns(NULL) / 1000000;
        for (Timer *timer = loop->timers; timer; timer = timer->next) {
            if (timer->deadline >= 0 && timer->deadline <= now + timeout)
                timeout = timer->deadline <= now ? 0 : (uint32_t)(timer->deadline - now);
        }
        int result = poll(loop->polls, (nfds_t)loop->poll_count, (int)timeout);
        if (result < 0 && errno == EINTR) continue;
        if (result < 0) break;
        if (loop->polls[0].revents & (POLLERR | POLLHUP | POLLNVAL)) loop->stop = 1;
        // Update libslirp's clock before incoming packets create flows. Otherwise
        // the first DNS socket expires immediately against the phone's uptime.
        if (!loop->stop) slirp_pollfds_poll(loop->slirp, 0, get_revents, loop);
        if (!loop->stop && (loop->polls[0].revents & POLLIN)) read_packets(loop);
        flush_output(loop);
        for (int fired = 0; fired < 32; fired++) {
            Timer *due = loop->timers;
            while (due && (due->deadline < 0 || due->deadline > clock_ns(NULL) / 1000000)) due = due->next;
            if (!due) break;
            due->deadline = -1;
            due->callback(due->opaque);
        }
    }
    if (loop->slirp) slirp_cleanup(loop->slirp);
    while (loop->timers) timer_free(loop->timers, loop);
    free(loop);
    nh_broker_end();
    close(fd);
    return;
invalid:
    (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/IllegalStateException"), "The isolated guest network could not start");
}
