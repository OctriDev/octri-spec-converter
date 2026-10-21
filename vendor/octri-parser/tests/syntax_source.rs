use octri_parser::{Limits, parse_raml_source, parse_source};

#[test]
fn generic_syntax_retains_locations_without_a_dialect() -> Result<(), Box<dyn std::error::Error>> {
    let source = parse_source(
        "memory://collection.json",
        "{\"item\":[],\"counter\":18446744073709551615}".into(),
        &Limits::default(),
    )?;
    assert_eq!(source.document["counter"].as_u64(), Some(u64::MAX));
    assert_eq!(source.locate("/item").column, Some(9));
    assert!(parse_source("memory://x.yaml", "a: 1\na: 2\n".into(), &Limits::default()).is_err());
    Ok(())
}

#[test]
fn raml_includes_are_inert_and_aliases_keep_include_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let text =
        "#%RAML 1.0\ntitle: Pets\ntypes: &types\n  Pet: !include types/pet.raml\ncopy: *types\n";
    let parsed = parse_raml_source("memory://api.raml", text.into(), &Limits::default())?;
    assert_eq!(parsed.includes["/types/Pet"], "types/pet.raml");
    assert_eq!(parsed.includes["/copy/Pet"], "types/pet.raml");
    assert_eq!(parsed.source.document["types"]["Pet"], "types/pet.raml");
    assert_eq!(parsed.source.locate("/types/Pet").line, Some(4));
    assert!(parse_source("memory://x.yaml", text.into(), &Limits::default()).is_err());
    assert!(
        parse_raml_source(
            "memory://x.raml",
            "x: !execute command\n".into(),
            &Limits::default()
        )
        .is_err()
    );
    assert!(
        parse_raml_source(
            "memory://x.raml",
            "x: !include {}\n".into(),
            &Limits::default()
        )
        .is_err()
    );
    Ok(())
}
