#include <dlfcn.h>
#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

JNIEXPORT void JNICALL
Java_io_github_gu1p_nodeharbor_NativeRuntime_redirectOutput(JNIEnv *env, jobject self, jint fd) {
    (void)self;
    if (fd < 3 || dup2(fd, STDOUT_FILENO) < 0 || dup2(fd, STDERR_FILENO) < 0)
        (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/IllegalStateException"), "The VM console descriptor is unavailable");
}

/* Loaded only from the installed APK, within SandboxService's isolated UID. */
static void *runtime_handle;
static void *runtime_symbol(JNIEnv *env, const char *symbol) {
    if (runtime_handle == NULL) runtime_handle = dlopen("libqemu-system-aarch64.so", RTLD_NOW | RTLD_LOCAL);
    void *address = runtime_handle == NULL ? NULL : dlsym(runtime_handle, symbol);
    if (address == NULL) (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/IllegalStateException"), "The packaged VM runtime could not be loaded");
    return address;
}

JNIEXPORT jstring JNICALL
Java_io_github_gu1p_nodeharbor_NativeRuntime_version(JNIEnv *env, jobject self) {
    (void)self;
    const char *(*version)(void) = (const char *(*)(void))runtime_symbol(env, "nodeharbor_qemu_version");
    if (version == NULL) return NULL;
    return (*env)->NewStringUTF(env, version());
}

JNIEXPORT void JNICALL
Java_io_github_gu1p_nodeharbor_NativeRuntime_run(JNIEnv *env, jobject self, jobjectArray arguments) {
    (void)self;
    int (*entry)(int, char **) = (int (*)(int, char **))runtime_symbol(env, "nodeharbor_qemu_main");
    if (entry == NULL) return;
    jsize count = arguments == NULL ? 0 : (*env)->GetArrayLength(env, arguments);
    if (count < 1 || count > 80) goto invalid;
    char *argv[81] = {0};
    for (jsize index = 0; index < count; index++) {
        jstring argument = (jstring)(*env)->GetObjectArrayElement(env, arguments, index);
        if (argument == NULL || (*env)->GetStringUTFLength(env, argument) > 4096) goto cleanup;
        const char *text = (*env)->GetStringUTFChars(env, argument, NULL);
        if (text == NULL) goto cleanup;
        argv[index] = strdup(text);
        (*env)->ReleaseStringUTFChars(env, argument, text);
        (*env)->DeleteLocalRef(env, argument);
        if (argv[index] == NULL) goto cleanup;
    }
    entry((int)count, argv); /* QEMU exits this isolated process on shutdown. */
cleanup:
    for (jsize index = 0; index < count; index++) free(argv[index]);
    if ((*env)->ExceptionCheck(env)) return;
invalid:
    (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/IllegalStateException"), "The isolated VM runtime could not start");
}
