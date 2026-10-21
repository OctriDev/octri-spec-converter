package dev.octri.converter.kotlin

import com.fasterxml.jackson.databind.JsonNode

/** Kotlin facade over the same byte-array JNI bridge used by Java. */
class Converter(library: String, bridge: String) {
    private val native = dev.octri.converter.Converter(library, bridge)
    fun convert(request: JsonNode): JsonNode = native.convert(request)
    fun review(request: JsonNode): JsonNode = native.review(request)
    fun bundle(request: JsonNode): JsonNode = native.bundle(request)
    fun compare(before: JsonNode, after: JsonNode): JsonNode = native.compare(before, after)
    fun export(report: JsonNode, format: String = "json"): JsonNode = native.export(report, format)
    fun formats(): JsonNode = native.formats()
    fun detect(request: JsonNode): JsonNode = native.detect(request)
    fun batch(requests: JsonNode): JsonNode = native.batch(requests)
    fun call(envelope: JsonNode): JsonNode = native.call(envelope)
}
