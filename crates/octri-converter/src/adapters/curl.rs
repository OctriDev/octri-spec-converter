//! Parse cURL command text without invoking a shell or executing any command.
use super::http::{self, Auth, Header, Observation, Payload};
use crate::{Classification, Config, ConversionError, Result};
use crate::{
    source::{Sources, child},
    trace::Trace,
};
use serde_json::{Value, json};

#[derive(Debug)]
struct Token {
    text: String,
    offset: usize,
}

#[derive(Debug, Default)]
struct Command {
    method: Option<String>,
    urls: Vec<String>,
    headers: Vec<Header>,
    data: Vec<String>,
    json: bool,
    form: Vec<(String, bool)>,
    query: Vec<(String, String)>,
    get: bool,
    head: bool,
    auth: Auth,
}

pub(crate) fn convert(sources: &mut Sources, config: &Config, trace: &mut Trace) -> Result<Value> {
    let text = sources.text(&sources.root)?;
    let commands = lex(text)?;
    let mut builder = http::Builder::new("Converted cURL requests", sources, config, trace);
    for (index, tokens) in commands.iter().enumerate() {
        let pointer = child("/commands", &index.to_string());
        for (part, command) in parse(tokens, &pointer, &mut builder)?
            .into_iter()
            .enumerate()
        {
            let pointer = child(&pointer, &part.to_string());
            emit(command, &pointer, &mut builder)?;
        }
    }
    Ok(builder.finish())
}

fn lex(text: &str) -> Result<Vec<Vec<Token>>> {
    let mut commands = Vec::new();
    let mut command = Vec::new();
    let mut token = String::new();
    let mut token_start = None;
    let mut quote = None;
    let mut chars = text.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
                continue;
            }
            if delimiter == '"' && matches!(ch, '$' | '`') {
                return Err(ConversionError::new(
                    "SHELL_EXPANSION",
                    format!("Shell expansion at byte {offset} requires an explicit literal value"),
                ));
            }
            if delimiter == '"' && ch == '\\' {
                let Some((_, next)) = chars.next() else {
                    return Err(ConversionError::new("CURL_SYNTAX", "Trailing escape"));
                };
                if next == '\n' {
                    continue;
                }
                if matches!(next, '"' | '\\' | '$' | '`') {
                    token.push(next);
                } else {
                    token.push('\\');
                    token.push(next);
                }
                continue;
            }
            token.push(ch);
            continue;
        }
        match ch {
            '\'' | '"' => {
                token_start.get_or_insert(offset);
                quote = Some(ch);
            }
            '\\' => {
                let Some((_, next)) = chars.next() else {
                    return Err(ConversionError::new("CURL_SYNTAX", "Trailing escape"));
                };
                if next == '\n' {
                    continue;
                }
                if next == '\r' && chars.peek().is_some_and(|(_, ch)| *ch == '\n') {
                    chars.next();
                    continue;
                }
                token_start.get_or_insert(offset);
                token.push(next);
            }
            '$' | '`' | '|' | '&' | '<' | '>' => {
                return Err(ConversionError::new(
                    "SHELL_SYNTAX",
                    format!(
                        "Shell execution/redirection at byte {offset} is not accepted. Supply literal cURL arguments."
                    ),
                ));
            }
            '#' if token_start.is_none() => {
                while chars.peek().is_some_and(|(_, ch)| *ch != '\n') {
                    chars.next();
                }
            }
            ch if ch.is_whitespace() || ch == ';' => {
                if let Some(start) = token_start.take() {
                    command.push(Token {
                        text: std::mem::take(&mut token),
                        offset: start,
                    });
                }
                if (ch == '\n' || ch == ';') && !command.is_empty() {
                    commands.push(std::mem::take(&mut command));
                }
            }
            _ => {
                token_start.get_or_insert(offset);
                token.push(ch);
            }
        }
    }
    if quote.is_some() {
        return Err(ConversionError::new(
            "CURL_SYNTAX",
            "Unterminated quoted argument",
        ));
    }
    if let Some(offset) = token_start {
        command.push(Token {
            text: token,
            offset,
        });
    }
    if !command.is_empty() {
        commands.push(command);
    }
    if commands.is_empty() {
        return Err(ConversionError::new(
            "CURL_SYNTAX",
            "No cURL command supplied",
        ));
    }
    Ok(commands)
}

fn parse(tokens: &[Token], pointer: &str, builder: &mut http::Builder<'_>) -> Result<Vec<Command>> {
    if tokens
        .first()
        .is_none_or(|token| !matches!(token.text.as_str(), "curl" | "curl.exe"))
    {
        return Err(ConversionError::new(
            "CURL_COMMAND",
            "Every command must begin with curl; no other executable is accepted",
        ));
    }
    let mut results = Vec::new();
    let mut command = Command::default();
    let mut index = 1;
    let mut positional = false;
    while index < tokens.len() {
        let token = &tokens[index];
        let raw = token.text.as_str();
        index += 1;
        if positional || !raw.starts_with('-') {
            command.urls.push(raw.into());
            continue;
        }
        if raw == "--" {
            positional = true;
            continue;
        }
        let (flag, attached) = if raw.starts_with("--") {
            raw.split_once('=')
                .map_or((raw, None), |(name, value)| (name, Some(value)))
        } else if raw.len() > 2
            && matches!(
                raw.as_bytes()[1],
                b'X' | b'H' | b'd' | b'F' | b'u' | b'b' | b'A' | b'e'
            )
        {
            (&raw[..2], Some(&raw[2..]))
        } else {
            (raw, None)
        };
        if flag == "--next" {
            results.push(std::mem::take(&mut command));
            continue;
        }
        if matches!(flag, "-G" | "--get") {
            command.get = true;
            continue;
        }
        if matches!(flag, "-I" | "--head") {
            command.head = true;
            continue;
        }
        if matches!(flag, "--basic" | "--digest" | "--ntlm" | "--negotiate") {
            if matches!(flag, "--basic" | "--digest") {
                command.auth = Auth::scheme(
                    "curlAuth",
                    json!({"type":"http","scheme":if flag=="--basic"{"basic"}else{"digest"}}),
                );
            } else {
                builder.record(
                    pointer,
                    &[],
                    "AUTH_UNREPRESENTABLE",
                    Classification::Blocked,
                    format!("{flag} requires an explicit OpenAPI security mapping"),
                    None,
                );
            }
            continue;
        }
        if matches!(
            flag,
            "-k" | "--insecure"
                | "-L"
                | "--location"
                | "--compressed"
                | "-s"
                | "--silent"
                | "-S"
                | "--show-error"
                | "-v"
                | "--verbose"
                | "-i"
                | "--include"
                | "--fail"
                | "--fail-with-body"
                | "--globoff"
                | "--http1.0"
                | "--http1.1"
                | "--http2"
                | "--http3"
                | "--http3-only"
                | "--no-progress-meter"
        ) {
            builder.record(
                pointer,
                &[],
                "CURL_TRANSPORT_OPTION",
                Classification::Omitted,
                format!("{flag} controls cURL runtime behavior; retained in original command"),
                None,
            );
            continue;
        }
        let recognized = matches!(
            flag,
            "-X" | "--request"
                | "-H"
                | "--header"
                | "-d"
                | "--data"
                | "--data-raw"
                | "--data-binary"
                | "--data-ascii"
                | "--data-urlencode"
                | "--json"
                | "-F"
                | "--form"
                | "--form-string"
                | "--url"
                | "--url-query"
                | "-u"
                | "--user"
                | "--oauth2-bearer"
                | "-b"
                | "--cookie"
                | "-A"
                | "--user-agent"
                | "-e"
                | "--referer"
                | "--connect-timeout"
                | "--max-time"
                | "--retry"
                | "--proxy"
                | "--resolve"
                | "--connect-to"
                | "--cacert"
                | "--cert"
                | "--key"
                | "-o"
                | "--output"
        );
        if !recognized {
            return Err(ConversionError::new(
                "CURL_OPTION",
                format!(
                    "Unsupported option {flag} at byte {}. No conversion is claimed for unparsed options.",
                    token.offset
                ),
            ));
        }
        let value = if let Some(value) = attached {
            value
        } else {
            let value = tokens.get(index).ok_or_else(|| {
                ConversionError::new("CURL_ARGUMENT", format!("{flag} needs an argument"))
            })?;
            index += 1;
            value.text.as_str()
        };
        match flag{
            "-X"|"--request"=>command.method=Some(value.into()),
            "--url"=>command.urls.push(value.into()),
            "-H"|"--header"=>{
                if value.starts_with('@'){return Err(ConversionError::new("CURL_HEADER_FILE","Header files must be expanded explicitly into literal header arguments"));}
                let (name,value)=value.split_once(':').ok_or_else(||ConversionError::new("CURL_HEADER","Header argument needs name:value"))?;
                if value.trim().is_empty(){builder.record(pointer,&[],"CURL_HEADER_REMOVAL",Classification::Omitted,"Empty cURL header removes a client default; retained in source",Some(&json!(name)));}
                else{command.headers.push(Header{name:name.trim().into(),value:value.trim().into(),pointer:pointer.into()});}
            },
            "-d"|"--data"|"--data-raw"|"--data-binary"|"--data-ascii"|"--json"=>{
                if value.starts_with('@')&&flag!="--data-raw"{return Err(ConversionError::new("CURL_DATA_FILE","Supply file content explicitly as a literal body; @file and stdin never grant filesystem access"));}
                command.data.push(value.into());command.json|=flag=="--json";
            },
            "--data-urlencode"|"--url-query"=>{
                if value.contains('@')&&!value.contains('='){return Err(ConversionError::new("CURL_DATA_FILE","File-based URL encoding requires explicit literal content"));}
                let (name,value)=value.split_once('=').unwrap_or(("",value));
                if flag=="--url-query"{command.query.push((name.into(),value.into()));}
                else{command.data.push(url::form_urlencoded::Serializer::new(String::new()).append_pair(name,value).finish());}
            },
            "-F"|"--form"|"--form-string"=>command.form.push((value.into(),flag=="--form-string")),
            "-u"|"--user"=>command.auth=Auth::scheme("basicAuth",json!({"type":"http","scheme":"basic"})),
            "--oauth2-bearer"=>command.auth=Auth::scheme("bearerAuth",json!({"type":"http","scheme":"bearer"})),
            "-b"|"--cookie"|"-A"|"--user-agent"|"-e"|"--referer"=>command.headers.push(Header{name:match flag{"-b"|"--cookie"=>"Cookie","-A"|"--user-agent"=>"User-Agent",_=>"Referer"}.into(),value:value.into(),pointer:pointer.into()}),
            _=>builder.record(pointer,&[],"CURL_TRANSPORT_OPTION",Classification::Omitted,format!("{flag} runtime setting retained in original command; no network, file read or file write occurs"),Some(&json!(value))),
        }
    }
    results.push(command);
    Ok(results)
}

fn emit(mut command: Command, pointer: &str, builder: &mut http::Builder<'_>) -> Result<()> {
    if command.urls.is_empty() {
        return Err(ConversionError::new(
            "CURL_URL",
            "Each cURL transfer requires a URL",
        ));
    }
    if !command.form.is_empty() && !command.data.is_empty() {
        return Err(ConversionError::new(
            "CURL_BODY",
            "Form and data body options cannot be combined",
        ));
    }
    let mut body = None;
    if !command.data.is_empty() {
        let text = command.data.join("&");
        if command.get {
            command.query.extend(
                url::form_urlencoded::parse(text.as_bytes())
                    .map(|(name, value)| (name.into_owned(), value.into_owned())),
            );
        } else {
            let media = command
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                .map(|header| header.value.as_str())
                .unwrap_or(if command.json {
                    "application/json"
                } else {
                    "application/x-www-form-urlencoded"
                });
            if media.starts_with("application/x-www-form-urlencoded") {
                let mut properties = serde_json::Map::new();
                let mut example = serde_json::Map::new();
                for (name, value) in url::form_urlencoded::parse(text.as_bytes()) {
                    if properties.contains_key(name.as_ref()) {
                        return Err(ConversionError::new(
                            "CURL_FORM",
                            "Repeated form fields need explicit array serialization",
                        ));
                    }
                    properties.insert(name.to_string(), json!({"type":"string"}));
                    example.insert(name.into_owned(), json!(value));
                }
                body = Some(Payload {
                    media: media.split(';').next().unwrap_or(media).into(),
                    schema: json!({"type":"object","properties":properties}),
                    example: Some(Value::Object(example)),
                    encoding: None,
                    pointer: pointer.into(),
                    inferred: true,
                });
            } else {
                body = http::example_payload(&text, Some(media), pointer, builder.sources)?;
            }
        }
    }
    if !command.form.is_empty() {
        let mut properties = serde_json::Map::new();
        let mut example = serde_json::Map::new();
        let mut encoding = serde_json::Map::new();
        for (raw, literal) in command.form {
            let (name, value) = raw.split_once('=').ok_or_else(|| {
                ConversionError::new("CURL_FORM", "Form argument requires name=value")
            })?;
            if name.is_empty() || properties.contains_key(name) {
                return Err(ConversionError::new(
                    "CURL_FORM",
                    "Empty or repeated form field requires explicit mapping",
                ));
            }
            let mut parts = value.split(';');
            let value = parts.next().unwrap_or("");
            let is_file = !literal && value.starts_with('@');
            if !literal && value.starts_with('<') {
                return Err(ConversionError::new(
                    "CURL_FORM_FILE",
                    "Reading form field text from files requires explicit literal content",
                ));
            }
            properties.insert(
                name.into(),
                if is_file {
                    json!({"type":"string","format":"binary"})
                } else {
                    json!({"type":"string"})
                },
            );
            if is_file {
                builder.record(
                    pointer,
                    &[],
                    "FILE_CONTENT_NOT_READ",
                    Classification::Transformed,
                    "Upload path represented by binary schema; file never read",
                    Some(&json!(value)),
                );
            } else {
                example.insert(
                    name.into(),
                    json!(if literal {
                        raw.split_once('=').map(|(_, value)| value).unwrap_or("")
                    } else {
                        value
                    }),
                );
            }
            if !literal {
                for modifier in parts {
                    if let Some(media) = modifier.strip_prefix("type=") {
                        encoding.insert(name.into(), json!({"contentType":media}));
                    } else {
                        builder.record(
                            pointer,
                            &[],
                            "CURL_FORM_MODIFIER",
                            Classification::Omitted,
                            "Form modifier retained in original command; review encoding",
                            Some(&json!(modifier)),
                        );
                    }
                }
            }
        }
        body = Some(Payload {
            media: "multipart/form-data".into(),
            schema: json!({"type":"object","properties":properties}),
            example: Some(Value::Object(example)),
            encoding: if encoding.is_empty() {
                None
            } else {
                Some(Value::Object(encoding))
            },
            pointer: pointer.into(),
            inferred: true,
        });
    }
    let method = command.method.unwrap_or_else(|| {
        if command.head {
            "HEAD"
        } else if command.get {
            "GET"
        } else if body.is_some() {
            "POST"
        } else {
            "GET"
        }
        .into()
    });
    for (index, mut url) in command.urls.into_iter().enumerate() {
        if !command.query.is_empty() {
            let mut parsed = url::Url::parse(&url)
                .map_err(|error| ConversionError::new("CURL_URL", error.to_string()))?;
            parsed.query_pairs_mut().extend_pairs(&command.query);
            url = parsed.to_string();
        }
        builder.add(Observation {
            pointer: child(pointer, &index.to_string()),
            name: format!("{method} {}", url.split('?').next().unwrap_or(&url)),
            description: None,
            method: method.clone(),
            url,
            headers: command.headers.clone(),
            path_variables: Default::default(),
            body: body.clone(),
            responses: Vec::new(),
            tags: Vec::new(),
            auth: command.auth.clone(),
        })?;
    }
    Ok(())
}
