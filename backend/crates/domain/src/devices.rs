//! Devices of sessions (L02-US08): the browser and the operating system a
//! `User-Agent` names, so that a member recognizes their sessions in the
//! list ("Firefox · Linux"). A guess, never a security check: the header is
//! whatever the client sends.

/// Longest `User-Agent` kept with a session, in bytes.
pub const USER_AGENT_MAX_BYTES: usize = 512;

/// What a `User-Agent` tells about the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Device {
    pub browser: Option<&'static str>,
    pub os: Option<&'static str>,
}

/// The device of `user_agent`; `None` parts when unrecognized.
pub fn device(user_agent: &str) -> Device {
    Device {
        browser: browser(user_agent),
        os: os(user_agent),
    }
}

/// Browsers by their token, most specific first: Edge and Opera also say
/// "Chrome", Chrome also says "Safari".
const BROWSERS: [(&str, &str); 9] = [
    ("Edg/", "Edge"),
    ("EdgiOS/", "Edge"),
    ("OPR/", "Opera"),
    ("SamsungBrowser/", "Samsung Internet"),
    ("Firefox/", "Firefox"),
    ("FxiOS/", "Firefox"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
    ("Safari/", "Safari"),
];

/// Systems by their token, most specific first: iOS says "like Mac OS X",
/// Android and ChromeOS say "Linux".
const SYSTEMS: [(&str, &str); 8] = [
    ("iPhone", "iOS"),
    ("iPad", "iPadOS"),
    ("Android", "Android"),
    ("CrOS", "ChromeOS"),
    ("Windows", "Windows"),
    ("Macintosh", "macOS"),
    ("Mac OS X", "macOS"),
    ("Linux", "Linux"),
];

fn browser(user_agent: &str) -> Option<&'static str> {
    BROWSERS
        .iter()
        .find(|(token, _)| user_agent.contains(token))
        .map(|(_, name)| *name)
}

fn os(user_agent: &str) -> Option<&'static str> {
    SYSTEMS
        .iter()
        .find(|(token, _)| user_agent.contains(token))
        .map(|(_, name)| *name)
}

/// `user_agent` as kept with a session: trimmed, at most
/// [`USER_AGENT_MAX_BYTES`] (on a character boundary); `None` when empty.
pub fn kept_user_agent(user_agent: &str) -> Option<String> {
    let trimmed = user_agent.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut end = trimmed.len().min(USER_AGENT_MAX_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(trimmed[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(user_agent: &str) -> (Option<&'static str>, Option<&'static str>) {
        let d = device(user_agent);
        (d.browser, d.os)
    }

    #[test]
    fn common_browsers_and_systems_are_recognized() {
        let cases = [
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
                (Some("Firefox"), Some("Linux")),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
                (Some("Chrome"), Some("Windows")),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
                (Some("Edge"), Some("Windows")),
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
                (Some("Safari"), Some("macOS")),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
                (Some("Safari"), Some("iOS")),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1",
                (Some("Chrome"), Some("iOS")),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36",
                (Some("Chrome"), Some("Android")),
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 OPR/115.0.0.0",
                (Some("Opera"), Some("ChromeOS")),
            ),
        ];
        for (user_agent, expected) in cases {
            assert_eq!(named(user_agent), expected, "{user_agent}");
        }
    }

    #[test]
    fn unknown_clients_name_nothing() {
        assert_eq!(named("curl/8.11.0"), (None, None));
        assert_eq!(named(""), (None, None));
    }

    #[test]
    fn user_agents_are_kept_trimmed_and_bounded() {
        assert_eq!(
            kept_user_agent("  Firefox/133.0 "),
            Some("Firefox/133.0".to_owned())
        );
        assert_eq!(kept_user_agent("   "), None);
        let long = "é".repeat(400);
        let kept = kept_user_agent(&long).unwrap_or_default();
        assert!(kept.len() <= USER_AGENT_MAX_BYTES);
        assert_eq!(kept.chars().count(), USER_AGENT_MAX_BYTES / 2);
    }
}
