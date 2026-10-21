import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import dev.octri.converter.Converter;

public final class BindingCheck {
  public static void main(String[] args) throws Exception {
    var json = new ObjectMapper().enable(com.fasterxml.jackson.databind.DeserializationFeature.USE_BIG_INTEGER_FOR_INTS).enable(com.fasterxml.jackson.databind.DeserializationFeature.USE_BIG_DECIMAL_FOR_FLOATS);
    var envelope = json.readTree(System.in);
    var converter = new Converter(args[0], args[1]);
    String action = envelope.path("action").asText();
    if (!"1".equals(System.getenv("OCTRI_BINDING_RAW")) && envelope.path("abiVersion").asInt() == 1 && java.util.List.of("convert", "review", "bundle", "compare", "export", "formats", "detect", "batch").contains(action)) {
      ObjectNode result = json.createObjectNode();
      com.fasterxml.jackson.databind.JsonNode value;
      switch (action) {
        case "convert": value = converter.convert(envelope.get("request")); break;
        case "review": value = converter.review(envelope.get("request")); break;
        case "bundle": value = converter.bundle(envelope.get("request")); break;
        case "detect": value = converter.detect(envelope.get("request")); break;
        case "batch": value = converter.batch(envelope.get("requests")); break;
        case "compare": value = converter.compare(envelope.get("before"), envelope.get("after")); break;
        case "export": value = converter.export(envelope.get("report"), envelope.get("format").asText()); break;
        default: value = converter.formats();
      }
      result.set("Ok", value);
      json.writeValue(System.out, result);
    } else {
      json.writeValue(System.out, converter.call(envelope));
    }
  }
}
