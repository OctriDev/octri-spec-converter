/* JNI owns only byte-array transport. Rust retains semantics and allocations. */
#include <jni.h>
#include <limits.h>
#include "octri_converter.h"

JNIEXPORT jint JNICALL Java_dev_octri_converter_NativeBridge_abiVersion(JNIEnv *env, jclass type) {
    (void)env; (void)type;
    return (jint)octri_converter_abi_version();
}

static void fail(JNIEnv *env, const char *message) {
    jclass type = (*env)->FindClass(env, "java/lang/IllegalStateException");
    if (type != NULL) (*env)->ThrowNew(env, type, message);
}

JNIEXPORT jbyteArray JNICALL Java_dev_octri_converter_NativeBridge_call(JNIEnv *env, jclass type, jbyteArray input) {
    (void)type;
    if (input == NULL) { fail(env, "Native request cannot be null"); return NULL; }
    jsize length = (*env)->GetArrayLength(env, input);
    jbyte *bytes = (*env)->GetByteArrayElements(env, input, NULL);
    if (bytes == NULL) return NULL; /* JVM has an allocation exception pending. */
    OctriConverterBuffer *result = octri_converter_call((const uint8_t *)bytes, (size_t)length);
    (*env)->ReleaseByteArrayElements(env, input, bytes, JNI_ABORT);
    if (result == NULL) { fail(env, "Native converter returned a null buffer"); return NULL; }
    size_t size = octri_converter_buffer_len(result);
    jbyteArray output = NULL;
    if (size > INT_MAX) {
        fail(env, "Native response exceeds JVM array limit");
    } else {
        output = (*env)->NewByteArray(env, (jsize)size);
        if (output != NULL) (*env)->SetByteArrayRegion(env, output, 0, (jsize)size, (const jbyte *)octri_converter_buffer_data(result));
    }
    octri_converter_buffer_free(result);
    return output;
}
