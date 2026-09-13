#ifndef NODEHARBOR_SOCKET_ADAPTER_H
#define NODEHARBOR_SOCKET_ADAPTER_H
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <unistd.h>
#include <fcntl.h>
#include <jni.h>

int nh_broker_begin(JNIEnv *env, jobject bridge);
void nh_broker_end(void);
int nh_socket(int domain, int type, int protocol);
int nh_close(int fd);
int nh_connect(int fd, const struct sockaddr *address, socklen_t length);
int nh_bind(int fd, const struct sockaddr *address, socklen_t length);
int nh_getpeername(int fd, struct sockaddr *address, socklen_t *length);
int nh_getsockname(int fd, struct sockaddr *address, socklen_t *length);
int nh_getsockopt(int fd, int level, int option, void *value, socklen_t *length);
int nh_setsockopt(int fd, int level, int option, const void *value, socklen_t length);
int nh_ioctl(int fd, unsigned long request, void *value);
ssize_t nh_sendto(int fd, const void *data, size_t size, int flags, const struct sockaddr *address, socklen_t length);
ssize_t nh_recvfrom(int fd, void *data, size_t size, int flags, struct sockaddr *address, socklen_t *length);
ssize_t nh_recvmsg(int fd, struct msghdr *message, int flags);

/* Define only while compiling libslirp. Android continues to enforce the UID
 * boundary; these calls exchange explicit capabilities through Binder. */
#ifdef NODEHARBOR_SLIRP
#define socket nh_socket
#define close nh_close
#define connect nh_connect
#define bind nh_bind
#define getpeername nh_getpeername
#define getsockname nh_getsockname
#define getsockopt nh_getsockopt
#define setsockopt nh_setsockopt
#define ioctl nh_ioctl
#define sendto nh_sendto
#define recvfrom nh_recvfrom
#define recvmsg nh_recvmsg
#endif
#endif
