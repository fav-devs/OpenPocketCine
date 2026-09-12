//! Joining the camera's own network.
//!
//! Two halves, kept apart on purpose. [`JoinPolicy`] decides *whether to try again and
//! when*, using the core's deadlines, and is testable with no radio. [`command_line`]
//! builds the argument list for the platform's own tool, and is testable without running
//! it. Only the running is untestable, and it is three lines.
//!
//! The camera's network carries no internet. On a laptop that is an advantage rather
//! than a problem: keep Ethernet for the network and give Wi-Fi to the camera, with the
//! Wi-Fi interface metric set higher so routing prefers the wire.

use std::io;

/// What to do next about the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinStep {
    /// Leave this network first.
    Kick(String),
    /// Ask the platform to join.
    Join,
    /// Wait out the pause between attempts.
    Wait,
    /// On the camera's network, with an address on its subnet.
    Settled,
    /// Out of time.
    GaveUp(String),
}

/// The core's join deadlines. Read from the core rather than written down here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinTiming {
    pub deadline: f64,
    pub retry_pause: f64,
}

impl JoinTiming {
    /// The values the phones use.
    pub fn from_core() -> Self {
        // Safety: no arguments, no allocation.
        unsafe {
            Self {
                deadline: opc_core_sys::opc_join_deadline_seconds(),
                retry_pause: opc_core_sys::opc_join_retry_pause_seconds(),
            }
        }
    }
}

/// Decides when to try the join again.
#[derive(Debug)]
pub struct JoinPolicy {
    target: String,
    started: f64,
    last_attempt: Option<f64>,
    kicked: bool,
    timing: JoinTiming,
}

impl JoinPolicy {
    pub fn new(target: impl Into<String>, now: f64, timing: JoinTiming) -> Self {
        Self {
            target: target.into(),
            started: now,
            last_attempt: None,
            kicked: false,
            timing,
        }
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    /// Seconds left before the join is abandoned.
    pub fn seconds_left(&self, now: f64) -> f64 {
        (self.timing.deadline - (now - self.started)).max(0.0)
    }

    /// `current_ssid` is `None` when the platform will not say — which counts as being
    /// on target, because refusing to proceed on that basis strands an operator who is
    /// in fact connected.
    pub fn tick(&mut self, now: f64, current_ssid: Option<&str>, path_ready: bool) -> JoinStep {
        let on_target = match current_ssid {
            None => true,
            Some("") => true,
            Some(name) => name == self.target,
        };
        if on_target && path_ready {
            return JoinStep::Settled;
        }

        let left = self.seconds_left(now);
        if left <= 0.0 {
            return JoinStep::GaveUp(format!(
                "Could not join {}. Check the camera is awake and close by.",
                self.target
            ));
        }

        // Leave the current network once before the first attempt; a laptop that stays
        // associated elsewhere never gets a camera address.
        if !self.kicked {
            if let Some(name) = current_ssid.filter(|name| !name.is_empty() && *name != self.target)
            {
                self.kicked = true;
                return JoinStep::Kick(name.to_string());
            }
            self.kicked = true;
        }

        let due = match self.last_attempt {
            None => true,
            Some(at) => now - at >= self.timing.retry_pause,
        };
        // Do not start an attempt that cannot finish before the deadline.
        if due && left > self.timing.retry_pause {
            self.last_attempt = Some(now);
            return JoinStep::Join;
        }
        JoinStep::Wait
    }
}

/// The platform tool and its arguments for one network action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub program: String,
    pub arguments: Vec<String>,
}

/// Which platform's tooling to build for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// What to run to join `ssid`.
///
/// Windows needs a profile installed before it will connect, so joining is two commands;
/// [`profile_xml`] builds the document the first one takes.
pub fn command_line(
    platform: Platform,
    ssid: &str,
    password: &str,
    interface: Option<&str>,
) -> Vec<CommandLine> {
    let owned = |program: &str, arguments: &[&str]| CommandLine {
        program: program.to_string(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_string())
            .collect(),
    };
    match platform {
        Platform::Windows => {
            let mut connect = vec![
                "wlan".to_string(),
                "connect".to_string(),
                format!("name={ssid}"),
                format!("ssid={ssid}"),
            ];
            if let Some(name) = interface {
                connect.push(format!("interface={name}"));
            }
            vec![
                // The profile is added from a file the caller writes with `profile_xml`.
                owned("netsh", &["wlan", "add", "profile", "filename=%PROFILE%"]),
                CommandLine {
                    program: "netsh".to_string(),
                    arguments: connect,
                },
            ]
        }
        Platform::Linux => {
            let mut arguments = vec![
                "device".to_string(),
                "wifi".to_string(),
                "connect".to_string(),
                ssid.to_string(),
                "password".to_string(),
                password.to_string(),
            ];
            if let Some(name) = interface {
                arguments.push("ifname".to_string());
                arguments.push(name.to_string());
            }
            vec![CommandLine {
                program: "nmcli".to_string(),
                arguments,
            }]
        }
        Platform::MacOs => vec![CommandLine {
            program: "networksetup".to_string(),
            arguments: vec![
                "-setairportnetwork".to_string(),
                interface.unwrap_or("en0").to_string(),
                ssid.to_string(),
                password.to_string(),
            ],
        }],
    }
}

/// What to run to leave `ssid`.
pub fn disconnect_command(platform: Platform, ssid: &str) -> CommandLine {
    match platform {
        Platform::Windows => CommandLine {
            program: "netsh".to_string(),
            arguments: vec!["wlan".into(), "disconnect".into()],
        },
        Platform::Linux => CommandLine {
            program: "nmcli".to_string(),
            arguments: vec!["connection".into(), "down".into(), ssid.to_string()],
        },
        Platform::MacOs => CommandLine {
            program: "networksetup".to_string(),
            arguments: vec!["-setairportpower".into(), "en0".into(), "off".into()],
        },
    }
}

/// A Windows WLAN profile for a WPA2-Personal network.
///
/// `connectionMode` is manual: the laptop should not wander back onto the camera by
/// itself once the shoot is over.
pub fn profile_xml(ssid: &str, password: &str) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let name = escape(ssid);
    let key = escape(password);
    let hex: String = ssid
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    format!(
        r#"<?xml version="1.0"?>
<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
  <name>{name}</name>
  <SSIDConfig>
    <SSID>
      <hex>{hex}</hex>
      <name>{name}</name>
    </SSID>
  </SSIDConfig>
  <connectionType>ESS</connectionType>
  <connectionMode>manual</connectionMode>
  <MSM>
    <security>
      <authEncryption>
        <authentication>WPA2PSK</authentication>
        <encryption>AES</encryption>
        <useOneX>false</useOneX>
      </authEncryption>
      <sharedKey>
        <keyType>passPhrase</keyType>
        <protected>false</protected>
        <keyMaterial>{key}</keyMaterial>
      </sharedKey>
    </security>
  </MSM>
</WLANProfile>
"#
    )
}

/// What the shell has to supply: a way to see the current network, join one, and leave
/// one. Implementations run [`command_line`]; the tests fake them.
pub trait WifiJoiner {
    fn current_ssid(&self) -> Option<String>;
    fn local_ipv4_addresses(&self) -> Vec<String>;
    fn join(&mut self, ssid: &str, password: &str) -> io::Result<()>;
    fn disconnect(&mut self, ssid: &str) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> JoinTiming {
        JoinTiming {
            deadline: 90.0,
            retry_pause: 10.0,
        }
    }

    #[test]
    fn being_on_the_camera_network_with_an_address_is_settled() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        assert_eq!(
            policy.tick(1.0, Some("OsmoPocket4-ABC"), true),
            JoinStep::Settled
        );
    }

    #[test]
    fn the_right_network_without_an_address_is_not_settled_yet() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        // Associated but no DHCP lease on the camera's subnet: still joining.
        assert_ne!(
            policy.tick(1.0, Some("OsmoPocket4-ABC"), false),
            JoinStep::Settled
        );
    }

    #[test]
    fn a_hidden_network_name_counts_as_being_on_target() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        // Some platforms will not say which network they are on. Refusing to proceed
        // would strand an operator who is in fact connected.
        assert_eq!(policy.tick(1.0, None, true), JoinStep::Settled);
        assert_eq!(policy.tick(1.0, Some(""), true), JoinStep::Settled);
    }

    #[test]
    fn another_network_is_left_before_the_first_attempt() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        assert_eq!(
            policy.tick(1.0, Some("Studio 5GHz"), false),
            JoinStep::Kick("Studio 5GHz".to_string())
        );
        // Only once.
        assert_eq!(policy.tick(1.1, Some("Studio 5GHz"), false), JoinStep::Join);
    }

    #[test]
    fn attempts_are_spaced_by_the_cores_pause() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        assert_eq!(policy.tick(0.0, None, false), JoinStep::Join);
        assert_eq!(policy.tick(3.0, None, false), JoinStep::Wait);
        assert_eq!(policy.tick(9.9, None, false), JoinStep::Wait);
        assert_eq!(policy.tick(10.0, None, false), JoinStep::Join);
    }

    #[test]
    fn an_attempt_that_cannot_finish_in_time_is_not_started() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        policy.tick(0.0, None, false);
        // Five seconds left is less than the pause: waiting, not another doomed attempt.
        assert_eq!(policy.tick(85.0, None, false), JoinStep::Wait);
    }

    #[test]
    fn the_join_is_given_up_on_at_the_deadline() {
        let mut policy = JoinPolicy::new("OsmoPocket4-ABC", 0.0, timing());
        policy.tick(0.0, None, false);
        assert!(matches!(
            policy.tick(90.0, None, false),
            JoinStep::GaveUp(_)
        ));
        assert_eq!(policy.seconds_left(90.0), 0.0);
    }

    #[test]
    fn each_platform_gets_its_own_tool() {
        let windows = command_line(Platform::Windows, "Osmo", "secret", None);
        assert_eq!(
            windows.len(),
            2,
            "Windows installs a profile, then connects"
        );
        assert!(windows.iter().all(|line| line.program == "netsh"));

        let linux = command_line(Platform::Linux, "Osmo", "secret", None);
        assert_eq!(linux[0].program, "nmcli");
        assert!(linux[0].arguments.contains(&"secret".to_string()));

        let macos = command_line(Platform::MacOs, "Osmo", "secret", None);
        assert_eq!(macos[0].program, "networksetup");
        assert_eq!(macos[0].arguments[1], "en0");
    }

    #[test]
    fn a_named_interface_is_passed_through() {
        let linux = command_line(Platform::Linux, "Osmo", "secret", Some("wlan1"));
        assert!(linux[0]
            .arguments
            .windows(2)
            .any(|pair| pair == ["ifname".to_string(), "wlan1".to_string()]));

        let windows = command_line(Platform::Windows, "Osmo", "secret", Some("Wi-Fi 2"));
        assert!(windows[1]
            .arguments
            .iter()
            .any(|argument| argument == "interface=Wi-Fi 2"));
    }

    #[test]
    fn a_windows_profile_carries_the_network_in_both_forms() {
        let xml = profile_xml("Osmo", "secret");
        assert!(xml.contains("<name>Osmo</name>"));
        // Windows wants the name in hex as well.
        assert!(xml.contains("<hex>4F736D6F</hex>"));
        assert!(xml.contains("<keyMaterial>secret</keyMaterial>"));
        assert!(xml.contains("WPA2PSK"));
        // Manual, so the laptop does not wander back onto the camera later.
        assert!(xml.contains("<connectionMode>manual</connectionMode>"));
    }

    #[test]
    fn a_profile_escapes_names_that_would_break_the_document() {
        let xml = profile_xml("A&B <cam>", "pass\"word");
        assert!(xml.contains("A&amp;B &lt;cam&gt;"));
        assert!(xml.contains("pass&quot;word"));
        assert!(!xml.contains("<cam>"));
    }

    #[test]
    fn leaving_a_network_is_per_platform_too() {
        assert_eq!(
            disconnect_command(Platform::Linux, "Studio").arguments[2],
            "Studio"
        );
        assert_eq!(
            disconnect_command(Platform::Windows, "Studio").arguments[1],
            "disconnect"
        );
    }
}
