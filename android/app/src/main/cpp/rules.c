#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/types.h>

extern ssize_t nodeharbor_rules(const uint8_t *, size_t, uint8_t *, size_t);

JNIEXPORT jbyteArray JNICALL
Java_io_github_gu1p_nodeharbor_NativeRules_nativeRequest(JNIEnv *env, jobject self, jbyteArray input) {
    (void)self;
    const jsize limit = 1024 * 1024;
    if (input == NULL || (*env)->GetArrayLength(env, input) > limit) goto invalid;
    jsize length = (*env)->GetArrayLength(env, input);
    jbyte *bytes = (*env)->GetByteArrayElements(env, input, NULL);
    if (bytes == NULL) return NULL;
    uint8_t *output = malloc((size_t)limit);
    if (output == NULL) {
        (*env)->ReleaseByteArrayElements(env, input, bytes, JNI_ABORT);
        (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/OutOfMemoryError"), "Owner rule buffer unavailable");
        return NULL;
    }
    ssize_t size = nodeharbor_rules((const uint8_t *)bytes, (size_t)length, output, (size_t)limit);
    (*env)->ReleaseByteArrayElements(env, input, bytes, JNI_ABORT);
    if (size < 0 || size > limit) { free(output); goto invalid; }
    jbyteArray result = (*env)->NewByteArray(env, (jsize)size);
    if (result != NULL) (*env)->SetByteArrayRegion(env, result, 0, (jsize)size, (const jbyte *)output);
    free(output);
    return result;
invalid:
    (*env)->ThrowNew(env, (*env)->FindClass(env, "java/lang/IllegalArgumentException"), "Invalid owner rule request");
    return NULL;
}
