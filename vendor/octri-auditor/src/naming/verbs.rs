//! API action vocabulary. Match complete identifier words, never substrings:
//! `find` is an action, while `findings` and `finder` can name resources.
//!
//! This is deliberately an API dictionary rather than a general English POS
//! classifier: resource words such as `pet` must not become action prefixes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Read,
    Create,
    Update,
    Delete,
    Action,
}

pub fn family(word: &str) -> Option<Family> {
    match word {
        "count" | "describe" | "fetch" | "find" | "get" | "inspect" | "list" | "lookup"
        | "query" | "read" | "retrieve" | "scan" | "search" => Some(Family::Read),
        "add" | "create" | "insert" | "post" => Some(Family::Create),
        "edit" | "modify" | "patch" | "put" | "replace" | "set" | "update" => Some(Family::Update),
        "delete" | "destroy" | "erase" | "remove" => Some(Family::Delete),
        "accept" | "activate" | "approve" | "archive" | "assign" | "attach" | "authenticate"
        | "authorize" | "call" | "cancel" | "check" | "clear" | "clone" | "close" | "complete"
        | "connect" | "convert" | "copy" | "deactivate" | "decode" | "deny" | "detach"
        | "disable" | "disconnect" | "download" | "duplicate" | "enable" | "encode" | "execute"
        | "export" | "head" | "import" | "invite" | "invoke" | "link" | "load" | "lock"
        | "merge" | "move" | "open" | "options" | "parse" | "pause" | "preview" | "publish"
        | "receive" | "refresh" | "register" | "reject" | "rename" | "renew" | "reset"
        | "resolve" | "restart" | "restore" | "resume" | "retry" | "revoke" | "rotate" | "run"
        | "save" | "send" | "sign" | "split" | "start" | "stop" | "stream" | "submit"
        | "subscribe" | "sync" | "tag" | "trace" | "transform" | "trigger" | "unarchive"
        | "unassign" | "unlink" | "unlock" | "unpublish" | "unregister" | "unsubscribe"
        | "upload" | "validate" | "verify" | "watch" | "write" => Some(Family::Action),
        _ => None,
    }
}

/// Search/query/list/read can also describe resource nouns (search results,
/// query plans, list items, read receipts). Do not remove those after `get`.
pub fn verbal_read(word: &str) -> bool {
    matches!(word, "describe" | "fetch" | "find" | "inspect" | "retrieve")
}
