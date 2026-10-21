#ifndef OCTRI_CONVERTER_H
#define OCTRI_CONVERTER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct OctriConverterBuffer OctriConverterBuffer;
uint32_t octri_converter_abi_version(void);
/* Input buffers are zero-filled. Maximum input: 128 MiB. */
OctriConverterBuffer *octri_converter_buffer_new(size_t length);
/* Borrow input until call returns. Output belongs to this loaded library. */
OctriConverterBuffer *octri_converter_call(const uint8_t *data, size_t length);
/* Buffers have no NUL terminator. Write only newly allocated input buffers. */
uint8_t *octri_converter_buffer_data(OctriConverterBuffer *buffer);
size_t octri_converter_buffer_len(const OctriConverterBuffer *buffer);
/* Free every allocation exactly once, with no overlapping access. NULL is safe. */
void octri_converter_buffer_free(OctriConverterBuffer *buffer);
#ifdef __cplusplus
}
#endif
#endif
