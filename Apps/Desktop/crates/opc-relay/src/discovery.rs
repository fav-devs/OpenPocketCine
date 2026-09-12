//! Bonjour discovery of sharing hosts.
//!
//! Hosts are only ever found on the camera's own Wi-Fi. There is no peer-to-peer
//! discovery and no fallback to another network: a watcher that cannot see a host is
//! told to join the camera Wi-Fi rather than quietly reaching one somewhere else.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

use crate::ffi::ProtocolInfo;

/// A sharing host advertising on the shared Wi-Fi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredHost {
    pub name: String,
    pub camera: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
}

/// A host that advertises `w=0` is up but not offering a picture yet.
pub fn is_watchable(flag: Option<&str>) -> bool {
    flag != Some("0")
}

/// `Studio._opc-mon._tcp.local.` with the service type stripped back off.
pub fn instance_name(fullname: &str, service_type: &str) -> String {
    let suffix = format!(".{service_type}");
    fullname
        .strip_suffix(&suffix)
        .unwrap_or(fullname)
        .to_string()
}

#[derive(Debug)]
pub enum DiscoveryError {
    Mdns(mdns_sd::Error),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mdns(error) => write!(f, "could not browse for shared feeds: {error}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

impl From<mdns_sd::Error> for DiscoveryError {
    fn from(error: mdns_sd::Error) -> Self {
        Self::Mdns(error)
    }
}

/// Watches the shared Wi-Fi for hosts, keeping the newest record for each name.
pub struct Browser {
    daemon: ServiceDaemon,
    receiver: mdns_sd::Receiver<ServiceEvent>,
    service_type: String,
    camera_key: String,
    watchable_key: String,
    hosts: BTreeMap<String, DiscoveredHost>,
}

impl fmt::Debug for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Browser")
            .field("service_type", &self.service_type)
            .field("hosts", &self.hosts.len())
            .finish_non_exhaustive()
    }
}

impl Browser {
    pub fn start(info: &ProtocolInfo) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        let service_type = info.mdns_service_type();
        let receiver = daemon.browse(&service_type)?;
        Ok(Self {
            daemon,
            receiver,
            service_type,
            camera_key: info.txt_camera.clone(),
            watchable_key: info.txt_watchable.clone(),
            hosts: BTreeMap::new(),
        })
    }

    /// Drains events for up to `timeout` and returns the hosts known afterwards.
    pub fn poll(&mut self, timeout: Duration) -> Vec<DiscoveredHost> {
        let deadline = std::time::Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(event) = self.receiver.recv_timeout(remaining) else {
                break;
            };
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    if !is_watchable(info.get_property_val_str(&self.watchable_key)) {
                        self.hosts
                            .remove(&instance_name(info.get_fullname(), &self.service_type));
                        continue;
                    }
                    let name = instance_name(info.get_fullname(), &self.service_type);
                    let mut addresses: Vec<IpAddr> = info
                        .get_addresses()
                        .iter()
                        .map(|ip| ip.to_ip_addr())
                        .collect();
                    addresses.sort();
                    self.hosts.insert(
                        name.clone(),
                        DiscoveredHost {
                            name,
                            camera: info
                                .get_property_val_str(&self.camera_key)
                                .unwrap_or_default()
                                .to_string(),
                            addresses,
                            port: info.get_port(),
                        },
                    );
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    self.hosts
                        .remove(&instance_name(&fullname, &self.service_type));
                }
                _ => {}
            }
        }
        self.hosts()
    }

    pub fn hosts(&self) -> Vec<DiscoveredHost> {
        self.hosts.values().cloned().collect()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(&self.service_type);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_zero_hides_a_host() {
        assert!(is_watchable(None));
        assert!(is_watchable(Some("1")));
        assert!(is_watchable(Some("")));
        assert!(!is_watchable(Some("0")));
    }

    #[test]
    fn the_service_type_is_stripped_from_the_instance_name() {
        assert_eq!(
            instance_name("Studio iPhone._opc-mon._tcp.local.", "_opc-mon._tcp.local."),
            "Studio iPhone"
        );
    }

    #[test]
    fn an_unexpected_fullname_is_left_alone() {
        assert_eq!(
            instance_name("something-else", "_opc-mon._tcp.local."),
            "something-else"
        );
    }

    #[test]
    fn an_instance_name_may_contain_dots() {
        assert_eq!(
            instance_name("A.B iPhone._opc-mon._tcp.local.", "_opc-mon._tcp.local."),
            "A.B iPhone"
        );
    }
}
