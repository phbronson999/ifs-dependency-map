/// The parts of deploy.ini the dependency analysis needs.
#[derive(Debug, Default)]
pub struct DeployIni {
    pub component: Option<String>,
    /// (target component, STATIC|DYNAMIC, line)
    pub connections: Vec<(String, String, usize)>,
}

pub fn parse(text: &str) -> DeployIni {
    let mut out = DeployIni::default();
    let mut section = String::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match section.as_str() {
            "component" if k.eq_ignore_ascii_case("name") => {
                out.component = Some(v.to_ascii_uppercase());
            }
            "connections" if !k.is_empty() => {
                out.connections.push((k.to_ascii_uppercase(), v.to_ascii_uppercase(), i + 1));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_connections() {
        let d = super::parse("[Component]\nName=Shpord\n\n[Connections]\nMfgstd=STATIC\nCallc=DYNAMIC\n");
        assert_eq!(d.component.as_deref(), Some("SHPORD"));
        assert_eq!(d.connections[1], ("CALLC".into(), "DYNAMIC".into(), 6));
    }
}
