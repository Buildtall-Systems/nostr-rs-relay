pub enum EventResultStatus {
    Saved,
    Duplicate,
    Invalid,
    Blocked,
    RateLimited,
    Error,
    Restricted,
    AuthRequired,
}

pub struct EventResult {
    pub id: String,
    pub msg: String,
    pub status: EventResultStatus,
}

pub enum Notice {
    Message(String),
    EventResult(EventResult),
    AuthChallenge(String),
}

impl EventResultStatus {
    #[must_use]
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Duplicate | Self::Saved => true,
            Self::Invalid | Self::Blocked | Self::RateLimited | Self::Error | Self::Restricted | Self::AuthRequired => {
                false
            }
        }
    }

    #[must_use]
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Duplicate => "duplicate",
            Self::Invalid => "invalid",
            Self::Blocked => "blocked",
            Self::RateLimited => "rate-limited",
            Self::Error => "error",
            Self::Restricted => "restricted",
            Self::AuthRequired => "",
        }
    }
}

impl Notice {
    //pub fn err(err: error::Error, id: String) -> Notice {
    //    Notice::err_msg(format!("{}", err), id)
    //}

    #[must_use]
    pub fn message(msg: String) -> Notice {
        Notice::Message(msg)
    }

    fn prefixed(id: String, msg: &str, status: EventResultStatus) -> Notice {
        let msg = format!("{}: {}", status.prefix(), msg);
        Notice::EventResult(EventResult { id, msg, status })
    }

    #[must_use]
    pub fn invalid(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Invalid)
    }

    #[must_use]
    pub fn blocked(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Blocked)
    }

    #[must_use]
    pub fn rate_limited(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::RateLimited)
    }

    #[must_use]
    pub fn duplicate(id: String) -> Notice {
        Notice::prefixed(id, "", EventResultStatus::Duplicate)
    }

    #[must_use]
    pub fn error(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Error)
    }

    #[must_use]
    pub fn restricted(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Restricted)
    }

    #[must_use]
    pub fn saved(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            msg: "".into(),
            status: EventResultStatus::Saved,
        })
    }

    #[must_use]
    pub fn auth_required(id: String, msg: &str) -> Notice {
        Notice::EventResult(EventResult {
            id,
            msg: msg.to_string(),
            status: EventResultStatus::AuthRequired,
        })
    }

    /// A rejection whose text comes from the event admission service. Text
    /// that already carries a NIP-01 machine-readable prefix is kept verbatim;
    /// other text is prefixed `blocked: `. An admission denial never stores
    /// the event, so every status here answers OK false, `duplicate:` and
    /// `pow:` included.
    #[must_use]
    pub fn admission_denied(id: String, msg: &str) -> Notice {
        if msg.starts_with("auth-required:") {
            return Notice::auth_required(id, msg);
        }
        for (prefix, status) in ADMISSION_PREFIXES {
            if msg.starts_with(prefix) {
                return Notice::EventResult(EventResult {
                    id,
                    msg: msg.to_string(),
                    status,
                });
            }
        }
        Notice::blocked(id, msg)
    }
}

/// NIP-01 machine-readable prefixes passed through from an admission denial,
/// each with the rejecting status it reports.
const ADMISSION_PREFIXES: [(&str, EventResultStatus); 7] = [
    ("blocked:", EventResultStatus::Blocked),
    ("restricted:", EventResultStatus::Restricted),
    ("invalid:", EventResultStatus::Invalid),
    ("pow:", EventResultStatus::Blocked),
    ("duplicate:", EventResultStatus::Blocked),
    ("rate-limited:", EventResultStatus::RateLimited),
    ("error:", EventResultStatus::Error),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn result_of(notice: Notice) -> EventResult {
        match notice {
            Notice::EventResult(res) => res,
            _ => panic!("expected an event result"),
        }
    }

    #[test]
    fn admission_denied_keeps_prefixed_text() {
        for (prefix, _) in ADMISSION_PREFIXES {
            let msg = format!("{prefix} reason");
            let res = result_of(Notice::admission_denied("id".into(), &msg));
            assert_eq!(res.msg, msg);
            assert!(!res.status.to_bool(), "{prefix} must answer OK false");
        }
    }

    #[test]
    fn admission_denied_maps_statuses() {
        let res = result_of(Notice::admission_denied(
            "id".into(),
            "restricted: not ranked",
        ));
        assert!(matches!(res.status, EventResultStatus::Restricted));
        let res = result_of(Notice::admission_denied("id".into(), "error: down"));
        assert!(matches!(res.status, EventResultStatus::Error));
        let res = result_of(Notice::admission_denied(
            "id".into(),
            "rate-limited: slow down",
        ));
        assert!(matches!(res.status, EventResultStatus::RateLimited));
    }

    #[test]
    fn admission_denied_prefixes_plain_text() {
        let res = result_of(Notice::admission_denied("id".into(), "not allowed"));
        assert_eq!(res.msg, "blocked: not allowed");
        assert!(matches!(res.status, EventResultStatus::Blocked));
    }

    #[test]
    fn admission_denied_prefixes_empty_text() {
        let res = result_of(Notice::admission_denied("id".into(), ""));
        assert_eq!(res.msg, "blocked: ");
        assert!(!res.status.to_bool());
    }

    #[test]
    fn admission_denied_keeps_auth_required() {
        let msg = "auth-required: NIP-42 authentication required";
        let res = result_of(Notice::admission_denied("id".into(), msg));
        assert_eq!(res.msg, msg);
        assert!(matches!(res.status, EventResultStatus::AuthRequired));
    }
}
