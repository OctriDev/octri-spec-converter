package dev.octri.converter;

import com.fasterxml.jackson.databind.JsonNode;

public final class ConversionException extends RuntimeException {
  private static final long serialVersionUID = 1L;
  private final String convertCode;
  private final JsonNode diagnostics;

  public ConversionException(JsonNode error) {
    super(error.path("message").asText());
    convertCode = error.path("code").asText();
    diagnostics = error.path("diagnostics").deepCopy();
  }

  public String getConvertCode() {
    return convertCode;
  }

  public JsonNode getDiagnostics() {
    return diagnostics.deepCopy();
  }
}
