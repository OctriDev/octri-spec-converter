import com.fasterxml.jackson.databind.ObjectMapper
import dev.octri.converter.kotlin.Converter

fun main(args: Array<String>) {
    val json = ObjectMapper().enable(com.fasterxml.jackson.databind.DeserializationFeature.USE_BIG_INTEGER_FOR_INTS).enable(com.fasterxml.jackson.databind.DeserializationFeature.USE_BIG_DECIMAL_FOR_FLOATS)
    val envelope = json.readTree(System.`in`)
    val converter = Converter(args[0], args[1])
    val action = envelope.path("action").asText()
    val result = if (System.getenv("OCTRI_BINDING_RAW") != "1" && envelope.path("abiVersion").asInt() == 1 && action in listOf("convert", "review", "bundle", "compare", "export", "formats", "detect", "batch")) {
        val value = when (action) {
            "convert" -> converter.convert(envelope.get("request"))
            "review" -> converter.review(envelope.get("request"))
            "bundle" -> converter.bundle(envelope.get("request"))
            "detect" -> converter.detect(envelope.get("request"))
            "batch" -> converter.batch(envelope.get("requests"))
            "compare" -> converter.compare(envelope.get("before"), envelope.get("after"))
            "export" -> converter.export(envelope.get("report"), envelope.get("format").asText())
            else -> converter.formats()
        }
        json.createObjectNode().set<com.fasterxml.jackson.databind.JsonNode>("Ok", value)
    } else { converter.call(envelope) }
    json.writeValue(System.out, result)
}
