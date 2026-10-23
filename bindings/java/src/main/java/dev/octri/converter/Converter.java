package dev.octri.converter;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;

/** Stateless synchronous transport. Use an application executor for large converts. */
public final class Converter {
  private static final ObjectMapper JSON =
      new ObjectMapper()
          .enable(DeserializationFeature.USE_BIG_INTEGER_FOR_INTS)
          .enable(DeserializationFeature.USE_BIG_DECIMAL_FOR_FLOATS);

  /** Loads the native library pair packaged in this jar for the current platform. */
  public Converter() throws IOException {
    NativeBridge.loadBundled();
  }

  public Converter(String library, String bridge) {
    NativeBridge.load(library, bridge);
  }

  public JsonNode call(JsonNode envelope) throws IOException {
    byte[] input = JSON.writeValueAsBytes(envelope);
    if (input.length > 128 * 1024 * 1024) {
      throw new IllegalArgumentException("Converter envelope exceeds 128 MiB");
    }
    return JSON.readTree(NativeBridge.call(input));
  }

  private JsonNode result(ObjectNode envelope) throws IOException {
    envelope.put("abiVersion", 1);
    JsonNode output = call(envelope);
    if (output.has("Err")) {
      throw new ConversionException(output.get("Err"));
    }
    if (!output.has("Ok")) {
      throw new IOException("Invalid converter response envelope");
    }
    return output.get("Ok");
  }

  public JsonNode convert(JsonNode request) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "convert");
    envelope.set("request", request);
    return result(envelope);
  }

  public JsonNode review(JsonNode request) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "review");
    envelope.set("request", request);
    return result(envelope);
  }

  public JsonNode compare(JsonNode before, JsonNode after) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "compare");
    envelope.set("before", before);
    envelope.set("after", after);
    return result(envelope);
  }

  public JsonNode export(JsonNode report, String format) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "export").put("format", format);
    envelope.set("report", report);
    return result(envelope);
  }

  public JsonNode formats() throws IOException {
    return result(JSON.createObjectNode().put("action", "formats"));
  }

  public JsonNode detect(JsonNode request) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "detect");
    envelope.set("request", request);
    return result(envelope);
  }

  /** Requests must be an array; each returned item retains its Ok/Err envelope. */
  public JsonNode batch(JsonNode requests) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "batch");
    envelope.set("requests", requests);
    return result(envelope);
  }

  public JsonNode bundle(JsonNode request) throws IOException {
    ObjectNode envelope = JSON.createObjectNode().put("action", "bundle");
    envelope.set("request", request);
    return result(envelope);
  }

  public static ObjectNode text(String content, String uri) {
    return JSON.createObjectNode().put("uri", uri).put("content", content).put("generated", false);
  }
}
