//! Deferred input features must fail before producing an apparently usable API.
use octri_converter::{Config, ConversionRequest, InputFormat, convert, detect};
use octri_parser::Input;

#[test]
fn raml_composition_is_detected_and_rejected_without_partial_output()
-> Result<(), Box<dyn std::error::Error>> {
    for kind in ["Overlay", "Extension"] {
        let request = ConversionRequest {
            input: Input {
                uri: "memory://input/pasted.txt".into(),
                content: Some(format!(
                    "#%RAML 1.0 {kind}\nextends: api.raml\ntitle: Localized API\n/pets:\n  get: {{}}\n"
                )),
                generated: false,
            },
            files: Default::default(),
            config: Config::default(),
        };
        assert_eq!(detect(&request)?, InputFormat::Raml);
        let error = convert(&request).err().ok_or("unexpected conversion")?;
        assert_eq!(error.code, "RAML_COMPOSITION_UNSUPPORTED");
        assert!(error.message.contains("fully composed RAML 1.0"));
    }
    Ok(())
}
