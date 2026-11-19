//! Countries of addresses, from a CSV of ranges (`start,end,country`), and
//! the filter on who may reach the nexus at all.
//!
//! The CSV is what db-ip's "IP to Country Lite" and IP2Location's LITE DB1
//! are: addresses as text (`1.0.0.0`, `2001:db8::`) or as numbers, quoted or
//! not, the two-letter country code third. Lines that do not read are
//! skipped.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use anyhow::Context;
use ipnet::IpNet;

use crate::config::Config;

/// Sorted, non-overlapping ranges.
#[derive(Default)]
pub struct Geo {
    v4: Vec<(u32, u32, [u8; 2])>,
    v6: Vec<(u128, u128, [u8; 2])>,
}

impl Geo {
    pub fn load(path: &Path) -> anyhow::Result<Geo> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let geo = Geo::parse(&text);
        if geo.v4.is_empty() && geo.v6.is_empty() {
            anyhow::bail!("{}: no address ranges in it", path.display());
        }
        Ok(geo)
    }

    pub fn parse(text: &str) -> Geo {
        let mut geo = Geo::default();
        for line in text.lines() {
            let mut fields = line.split(',').map(|f| f.trim().trim_matches('"'));
            let (Some(start), Some(end), Some(country)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            let country = country.as_bytes();
            if country.len() != 2 || !country.iter().all(u8::is_ascii_alphabetic) {
                continue;
            }
            let cc = [country[0].to_ascii_uppercase(), country[1].to_ascii_uppercase()];
            match (address(start), address(end)) {
                (Some(Addr::V4(a)), Some(Addr::V4(b))) if a <= b => geo.v4.push((a, b, cc)),
                (Some(Addr::V6(a)), Some(Addr::V6(b))) if a <= b => geo.v6.push((a, b, cc)),
                // A number range (IP2Location) that is all IPv4.
                (Some(Addr::Number(a)), Some(Addr::Number(b))) if a <= b && b <= u32::MAX as u128 => {
                    geo.v4.push((a as u32, b as u32, cc))
                }
                (Some(Addr::Number(a)), Some(Addr::Number(b))) if a <= b => geo.v6.push((a, b, cc)),
                _ => {}
            }
        }
        geo.v4.sort_unstable_by_key(|r| r.0);
        geo.v6.sort_unstable_by_key(|r| r.0);
        geo
    }

    /// The country code of `ip`, if the database has it.
    pub fn country(&self, ip: IpAddr) -> Option<String> {
        let cc = match canonical(ip) {
            IpAddr::V4(v4) => find(&self.v4, u32::from(v4)),
            IpAddr::V6(v6) => find(&self.v6, u128::from(v6)),
        }?;
        Some(String::from_utf8_lossy(&cc).into_owned())
    }
}

fn find<T: Ord + Copy>(ranges: &[(T, T, [u8; 2])], n: T) -> Option<[u8; 2]> {
    let i = ranges.partition_point(|r| r.0 <= n);
    let (_, end, cc) = ranges.get(i.checked_sub(1)?)?;
    (n <= *end).then_some(*cc)
}

enum Addr {
    V4(u32),
    V6(u128),
    Number(u128),
}

fn address(s: &str) -> Option<Addr> {
    if let Ok(ip) = s.parse::<Ipv4Addr>() {
        return Some(Addr::V4(ip.into()));
    }
    if let Ok(ip) = s.parse::<Ipv6Addr>() {
        return Some(Addr::V6(ip.into()));
    }
    s.parse::<u128>().ok().map(Addr::Number)
}

/// An IPv4 address mapped into IPv6 as the IPv4 address.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Loopback, private and link-local addresses: always let in, never in a
/// GeoIP database.
pub fn is_local(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Who may reach the nexus: address ranges and countries, from the config.
pub struct Filter {
    networks: Vec<IpNet>,
    geo: Option<Geo>,
    countries: Vec<String>,
    allow_unknown: bool,
}

impl Filter {
    pub fn new(config: &Config) -> anyhow::Result<Filter> {
        let geo = match &config.geo.database {
            Some(path) => Some(Geo::load(path)?),
            None => None,
        };
        let countries: Vec<String> = config.geo.countries.iter().map(|c| c.trim().to_ascii_uppercase()).collect();
        if !countries.is_empty() && geo.is_none() {
            anyhow::bail!("geo.countries needs geo.database");
        }
        Ok(Filter { networks: config.networks(), geo, countries, allow_unknown: config.geo.allow_unknown })
    }

    /// The country of `ip`, if known.
    pub fn country(&self, ip: IpAddr) -> Option<String> {
        self.geo.as_ref()?.country(ip)
    }

    /// Whether `ip` (of `country`) may come in.
    pub fn allows(&self, ip: IpAddr, country: Option<&str>) -> bool {
        let ip = canonical(ip);
        if !self.networks.is_empty() && !self.networks.iter().any(|n| n.contains(&ip)) {
            return false;
        }
        if self.countries.is_empty() || is_local(ip) {
            return true;
        }
        match country {
            Some(c) => self.countries.iter().any(|allowed| allowed == c),
            None => self.allow_unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "1.0.0.0,1.0.0.255,AU\n\
                       \"16777472\",\"16778239\",\"CN\",\"China\"\n\
                       2.0.0.0,2.255.255.255,fr\n\
                       2001:db8::,2001:db8:ffff:ffff:ffff:ffff:ffff:ffff,ZZ\n\
                       2a00::,2a00:ffff:ffff:ffff:ffff:ffff:ffff:ffff,DE\n\
                       not,a,line\n";

    #[test]
    fn finds_countries() {
        let geo = Geo::parse(CSV);
        let at = |s: &str| geo.country(s.parse().unwrap());
        assert_eq!(at("1.0.0.7").as_deref(), Some("AU"));
        assert_eq!(at("1.0.1.1").as_deref(), Some("CN"));
        assert_eq!(at("2.3.4.5").as_deref(), Some("FR"));
        assert_eq!(at("::ffff:2.3.4.5").as_deref(), Some("FR"));
        assert_eq!(at("2a00:1::1").as_deref(), Some("DE"));
        assert_eq!(at("3.0.0.0"), None);
        assert_eq!(at("0.0.0.1"), None);
    }

    #[test]
    fn filters_by_country_and_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("geo.csv");
        std::fs::write(&path, CSV).unwrap();
        let mut config = Config::default();
        config.geo.database = Some(path);
        config.geo.countries = vec!["fr".into()];
        let filter = Filter::new(&config).unwrap();
        let check = |s: &str| {
            let ip: IpAddr = s.parse().unwrap();
            filter.allows(ip, filter.country(ip).as_deref())
        };
        assert!(check("2.3.4.5"));
        assert!(!check("1.0.0.7"));
        assert!(!check("3.0.0.0"));
        assert!(check("127.0.0.1"));
        assert!(check("192.168.1.2"));

        config.geo = Default::default();
        config.allowed_networks = vec!["198.51.100.0/24".into()];
        let filter = Filter::new(&config).unwrap();
        assert!(filter.allows("198.51.100.7".parse().unwrap(), None));
        assert!(!filter.allows("198.51.101.7".parse().unwrap(), None));
    }
}
