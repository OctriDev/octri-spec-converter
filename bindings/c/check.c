#include "octri_converter.h"
#include <assert.h>
#include <string.h>

int main(void) {
    assert(octri_converter_abi_version() == 1);
    assert(octri_converter_buffer_new(129u * 1024u * 1024u) == NULL);
    assert(octri_converter_buffer_data(NULL) == NULL);
    assert(octri_converter_buffer_len(NULL) == 0);
    octri_converter_buffer_free(NULL);
    const char *request = "{\"abiVersion\":1,\"action\":\"formats\"}";
    OctriConverterBuffer *output = octri_converter_call((const uint8_t *)request, strlen(request));
    assert(output != NULL);
    assert(octri_converter_buffer_len(output) > 100);
    assert(memcmp(octri_converter_buffer_data(output), "{\"Ok\":", 6) == 0);
    octri_converter_buffer_free(output);
    output = octri_converter_call(NULL, 1);
    assert(output != NULL);
    assert(memcmp(octri_converter_buffer_data(output), "{\"Err\":", 7) == 0);
    octri_converter_buffer_free(output);
    return 0;
}
