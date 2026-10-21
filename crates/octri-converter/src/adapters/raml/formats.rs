//! RAML format constraints that OpenAPI format annotations alone do not assert.
use crate::{ConversionError, Result};
use serde_json::{Value, json};

/// Return whether protocol-aware HTTP date validation still needs review.
pub(super) fn apply(schema: &mut Value, format: &Value) -> Result<bool> {
    let format = format
        .as_str()
        .ok_or_else(|| ConversionError::new("RAML_FORMAT", "Format must be a string"))?;
    if schema.get("format").and_then(Value::as_str) == Some("date-time") {
        match format {
            "rfc3339" => return Ok(false),
            "rfc2616" => {
                schema["format"] = json!("http-date");
                // RFC 2616 permits IMF-fixdate, RFC 850 and asctime forms.
                // Calendar validity and matching weekday remain runtime checks.
                let weekday = "(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)";
                let long_weekday = "(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)";
                let month = "(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)";
                let day = "(?:0[1-9]|[12][0-9]|3[01])";
                let time = "(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]";
                let pattern = format!(
                    "^(?:{weekday}, {day} {month} [0-9]{{4}} {time} GMT|{long_weekday}, {day}-{month}-[0-9]{{2}} {time} GMT|{weekday} {month} (?: [1-9]|[12][0-9]|3[01]) {time} [0-9]{{4}})$"
                );
                let constraint = json!({"type":"string","pattern":pattern});
                conjunction(schema, constraint);
                return Ok(true);
            }
            _ => {
                return Err(ConversionError::new(
                    "RAML_FORMAT",
                    "Datetime format must be rfc3339 or rfc2616",
                ));
            }
        }
    }
    let bounds: Option<(i64, i64)> = match format {
        "int8" => Some((i8::MIN.into(), i8::MAX.into())),
        "int16" => Some((i16::MIN.into(), i16::MAX.into())),
        "int" | "int32" => Some((i32::MIN.into(), i32::MAX.into())),
        "long" | "int64" => Some((i64::MIN, i64::MAX)),
        _ => None,
    };
    if let Some((minimum, maximum)) = bounds {
        // Keep authored bounds as a conjunction; neither constraint can widen
        // the other, including contradictory declarations and inherited types.
        conjunction(
            schema,
            json!({"type":"integer","minimum":minimum,"maximum":maximum}),
        );
    }
    schema["format"] = json!(match format {
        "int" => "int32",
        "long" => "int64",
        value => value,
    });
    Ok(false)
}

fn conjunction(schema: &mut Value, constraint: Value) {
    if let Some(items) = schema["allOf"].as_array_mut() {
        items.push(constraint);
    } else {
        schema["allOf"] = json!([constraint]);
    }
}
