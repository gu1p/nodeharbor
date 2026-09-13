#include "socket_adapter.h"
#include <errno.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    int fd, type;
    struct sockaddr_in peer;
    int connected;
} Capability;
static _Thread_local Capability capabilities[64];
static _Thread_local JNIEnv *jni;
static _Thread_local jobject broker;
static _Thread_local jmethodID open_tcp, open_udp, send_udp, close_flow, local_port;

static Capability *find(int fd) {
    for (size_t i = 0; i < 64; i++) if (capabilities[i].fd == fd) return &capabilities[i];
    return NULL;
}
static int exception(void) {
    if (!(*jni)->ExceptionCheck(jni)) return 0;
    (*jni)->ExceptionClear(jni);
    errno = EACCES;
    return -1;
}
int nh_broker_begin(JNIEnv *env, jobject bridge) {
    jni = env;
    broker = bridge;
    for (size_t i = 0; i < 64; i++) capabilities[i].fd = -1;
    jclass type = (*jni)->GetObjectClass(jni, bridge);
    open_tcp = (*jni)->GetMethodID(jni, type, "openTcp", "(I[BI)I");
    open_udp = (*jni)->GetMethodID(jni, type, "openUdp", "(I)I");
    send_udp = (*jni)->GetMethodID(jni, type, "sendUdp", "(I[BI[B)I");
    close_flow = (*jni)->GetMethodID(jni, type, "closeFlow", "(I)V");
    local_port = (*jni)->GetMethodID(jni, type, "localPort", "(I)I");
    (*jni)->DeleteLocalRef(jni, type);
    return exception();
}
void nh_broker_end(void) {
    for (size_t i = 0; i < 64; i++) if (capabilities[i].fd >= 0) nh_close(capabilities[i].fd);
    broker = NULL;
    jni = NULL;
}
static int adopt(int target, int source) {
    if (source < 0) { errno = EACCES; return -1; }
    int result = dup2(source, target);
    close(source);
    if (result < 0) return -1;
    if (fcntl(target, F_SETFD, FD_CLOEXEC) || fcntl(target, F_SETFL, O_NONBLOCK)) return -1;
    return 0;
}
int nh_socket(int domain, int type, int protocol) {
    int kind = type & ~(SOCK_CLOEXEC | SOCK_NONBLOCK);
    if (domain != AF_INET || (kind != SOCK_STREAM && kind != SOCK_DGRAM) ||
        (protocol != 0 && protocol != IPPROTO_TCP && protocol != IPPROTO_UDP)) {
        errno = EAFNOSUPPORT; return -1;
    }
    Capability *slot = find(-1);
    if (!slot || !broker) { errno = EMFILE; return -1; }
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) return -1;
    *slot = (Capability){.fd = fd, .type = kind};
    if (kind == SOCK_DGRAM) {
        int source = (*jni)->CallIntMethod(jni, broker, open_udp, fd);
        if (exception() || adopt(fd, source)) { nh_close(fd); return -1; }
    }
    return fd;
}
int nh_close(int fd) {
    Capability *slot = find(fd);
    if (slot) {
        (*jni)->CallVoidMethod(jni, broker, close_flow, fd);
        exception();
        slot->fd = -1;
    }
    return close(fd);
}
static int ipv4(const struct sockaddr *address, socklen_t length) {
    if (!address || length < sizeof(struct sockaddr_in) || address->sa_family != AF_INET) {
        errno = EAFNOSUPPORT; return -1;
    }
    return 0;
}
int nh_connect(int fd, const struct sockaddr *address, socklen_t length) {
    Capability *slot = find(fd);
    if (!slot || slot->type != SOCK_STREAM || ipv4(address, length)) { errno = EINVAL; return -1; }
    const struct sockaddr_in *peer = (const struct sockaddr_in *)address;
    jbyteArray ip = (*jni)->NewByteArray(jni, 4);
    if (!ip) return -1;
    (*jni)->SetByteArrayRegion(jni, ip, 0, 4, (const jbyte *)&peer->sin_addr);
    int source = (*jni)->CallIntMethod(jni, broker, open_tcp, fd, ip, ntohs(peer->sin_port));
    (*jni)->DeleteLocalRef(jni, ip);
    if (exception() || adopt(fd, source)) return -1;
    slot->peer = *peer;
    slot->connected = 1;
    return 0;
}
int nh_bind(int fd, const struct sockaddr *address, socklen_t length) {
    Capability *slot = find(fd);
    if (!slot || ipv4(address, length)) { errno = EINVAL; return -1; }
    const struct sockaddr_in *bind_address = (const struct sockaddr_in *)address;
    if (slot->type != SOCK_DGRAM || bind_address->sin_port || bind_address->sin_addr.s_addr) {
        errno = EOPNOTSUPP; return -1;
    }
    return 0; /* The broker already bound this UDP capability to an ephemeral port. */
}
static int copy_address(struct sockaddr *out, socklen_t *length, const struct sockaddr_in *value) {
    if (!out || !length) { errno = EINVAL; return -1; }
    size_t size = *length < sizeof(*value) ? *length : sizeof(*value);
    memcpy(out, value, size);
    *length = sizeof(*value);
    return 0;
}
int nh_getpeername(int fd, struct sockaddr *address, socklen_t *length) {
    Capability *slot = find(fd);
    if (!slot || !slot->connected) { errno = ENOTCONN; return -1; }
    return copy_address(address, length, &slot->peer);
}
int nh_getsockname(int fd, struct sockaddr *address, socklen_t *length) {
    if (!find(fd)) { errno = EBADF; return -1; }
    int port = (*jni)->CallIntMethod(jni, broker, local_port, fd);
    if (exception() || port < 1 || port > 65535) { errno = ENOTCONN; return -1; }
    struct sockaddr_in result = {.sin_family = AF_INET, .sin_port = htons((uint16_t)port)};
    return copy_address(address, length, &result);
}
int nh_getsockopt(int fd, int level, int option, void *value, socklen_t *length) {
    Capability *slot = find(fd);
    if (slot && level == SOL_SOCKET && (option == SO_ERROR || option == SO_TYPE)) {
        if (!value || !length || *length < sizeof(int)) { errno = EINVAL; return -1; }
        *(int *)value = option == SO_TYPE ? slot->type : 0;
        *length = sizeof(int);
        return 0;
    }
    return getsockopt(fd, level, option, value, length);
}
int nh_setsockopt(int fd, int level, int option, const void *value, socklen_t length) {
    (void)value; (void)length;
    if (!find(fd)) { errno = EBADF; return -1; }
    if (level == IPPROTO_TCP && option == TCP_NODELAY) return 0; /* Enabled by the broker. */
    errno = ENOPROTOOPT;
    return -1;
}
static int udp_size(int fd) {
    uint8_t header[12];
    ssize_t got = recv(fd, header, sizeof(header), MSG_PEEK | MSG_DONTWAIT);
    if (got < 12) { errno = got == 0 ? ECONNRESET : EAGAIN; return -1; }
    uint32_t length;
    memcpy(&length, header, 4);
    length = ntohl(length);
    if (length > 65507 || header[10] || header[11]) { errno = EPROTO; return -1; }
    return (int)length;
}
int nh_ioctl(int fd, unsigned long request, void *value) {
    Capability *slot = find(fd);
    if (slot && slot->type == SOCK_DGRAM && request == FIONREAD && value) {
        int size = udp_size(fd);
        if (size < 0) return -1;
        *(int *)value = size;
        return 0;
    }
    errno = ENOTTY;
    return -1;
}
ssize_t nh_sendto(int fd, const void *data, size_t size, int flags, const struct sockaddr *address, socklen_t length) {
    Capability *slot = find(fd);
    if (!slot || slot->type != SOCK_DGRAM || flags || size > 65507 || ipv4(address, length)) { errno = EINVAL; return -1; }
    const struct sockaddr_in *peer = (const struct sockaddr_in *)address;
    jbyteArray ip = (*jni)->NewByteArray(jni, 4);
    jbyteArray bytes = (*jni)->NewByteArray(jni, (jsize)size);
    if (!ip || !bytes) { if (ip) (*jni)->DeleteLocalRef(jni, ip); return -1; }
    (*jni)->SetByteArrayRegion(jni, ip, 0, 4, (const jbyte *)&peer->sin_addr);
    (*jni)->SetByteArrayRegion(jni, bytes, 0, (jsize)size, data);
    int result = (*jni)->CallIntMethod(jni, broker, send_udp, fd, ip, ntohs(peer->sin_port), bytes);
    (*jni)->DeleteLocalRef(jni, ip);
    (*jni)->DeleteLocalRef(jni, bytes);
    return exception() ? -1 : result;
}
ssize_t nh_recvfrom(int fd, void *data, size_t size, int flags, struct sockaddr *address, socklen_t *length) {
    Capability *slot = find(fd);
    if (!slot || slot->type != SOCK_DGRAM || flags) { errno = EINVAL; return -1; }
    int payload = udp_size(fd);
    if (payload < 0) return -1;
    uint8_t packet[65519];
    size_t total = (size_t)payload + 12;
    ssize_t peeked = recv(fd, packet, total, MSG_PEEK | MSG_DONTWAIT);
    if (peeked != (ssize_t)total) { errno = EAGAIN; return -1; }
    size_t offset = 0;
    while (offset < total) {
        ssize_t count = recv(fd, packet + offset, total - offset, MSG_DONTWAIT);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { errno = EPROTO; return -1; }
        offset += (size_t)count;
    }
    struct sockaddr_in peer = {.sin_family = AF_INET};
    memcpy(&peer.sin_addr, packet + 4, 4);
    memcpy(&peer.sin_port, packet + 8, 2);
    if (address && copy_address(address, length, &peer)) return -1;
    size_t copied = size < (size_t)payload ? size : (size_t)payload;
    memcpy(data, packet + 12, copied);
    return (ssize_t)copied;
}
ssize_t nh_recvmsg(int fd, struct msghdr *message, int flags) {
    if (flags & MSG_ERRQUEUE) { errno = EAGAIN; return -1; }
    return recvmsg(fd, message, flags);
}
