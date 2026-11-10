//! Design rules from a KiCad 9 `.kicad_pro` project file.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct NetClass {
    pub name: String,
    pub clearance: f64,
    pub track_width: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
}

impl Default for NetClass {
    fn default() -> Self {
        // KiCad's own defaults for a new board.
        NetClass { name: "Default".into(), clearance: 0.2, track_width: 0.25, via_diameter: 0.6, via_drill: 0.3 }
    }
}

#[derive(Debug, Clone)]
pub struct Rules {
    pub classes: Vec<NetClass>,
    /// (glob pattern, class index), checked in order.
    pub patterns: Vec<(String, usize)>,
    pub min_clearance: f64,
    pub min_track_width: f64,
    pub min_copper_edge_clearance: f64,
    pub min_hole_clearance: f64,
    pub min_hole_to_hole: f64,
    pub min_via_diameter: f64,
    pub min_through_hole: f64,
    pub min_via_annular: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            classes: vec![NetClass::default()],
            patterns: Vec::new(),
            min_clearance: 0.0,
            min_track_width: 0.0,
            min_copper_edge_clearance: 0.0,
            min_hole_clearance: 0.25,
            min_hole_to_hole: 0.25,
            min_via_diameter: 0.5,
            min_through_hole: 0.3,
            min_via_annular: 0.1,
        }
    }
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

impl Rules {
    pub fn parse(json: &str) -> Result<Rules, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("kicad_pro: {e}"))?;
        let mut r = Rules::default();
        if let Some(rules) = root.pointer("/board/design_settings/rules") {
            let set = |field: &mut f64, key: &str| {
                if let Some(x) = num(rules, key) {
                    *field = x;
                }
            };
            set(&mut r.min_clearance, "min_clearance");
            set(&mut r.min_track_width, "min_track_width");
            set(&mut r.min_copper_edge_clearance, "min_copper_edge_clearance");
            set(&mut r.min_hole_clearance, "min_hole_clearance");
            set(&mut r.min_hole_to_hole, "min_hole_to_hole");
            set(&mut r.min_via_diameter, "min_via_diameter");
            set(&mut r.min_through_hole, "min_through_hole_diameter");
            set(&mut r.min_via_annular, "min_via_annular_width");
        }
        if let Some(classes) = root.pointer("/net_settings/classes").and_then(|c| c.as_array()) {
            let base = NetClass::default();
            let mut out = Vec::new();
            for c in classes {
                let name = c.get("name").and_then(|n| n.as_str()).unwrap_or("Default").to_string();
                // A non-default class may leave fields unset; they fall back to Default's.
                let dflt = out.first().cloned().unwrap_or_else(|| base.clone());
                out.push(NetClass {
                    name,
                    clearance: num(c, "clearance").unwrap_or(dflt.clearance),
                    track_width: num(c, "track_width").unwrap_or(dflt.track_width),
                    via_diameter: num(c, "via_diameter").unwrap_or(dflt.via_diameter),
                    via_drill: num(c, "via_drill").unwrap_or(dflt.via_drill),
                });
            }
            // Keep Default first.
            if let Some(i) = out.iter().position(|c| c.name == "Default") {
                out.swap(0, i);
            }
            if !out.is_empty() {
                r.classes = out;
            }
        }
        if let Some(pats) = root.pointer("/net_settings/netclass_patterns").and_then(|c| c.as_array()) {
            for pt in pats {
                let (Some(pattern), Some(class)) = (pt.get("pattern").and_then(|x| x.as_str()), pt.get("netclass").and_then(|x| x.as_str())) else {
                    continue;
                };
                if let Some(ci) = r.classes.iter().position(|c| c.name == class) {
                    r.patterns.push((pattern.to_string(), ci));
                }
            }
        }
        Ok(r)
    }

    /// Net class index for a net name (0 = Default).
    pub fn class_of(&self, net_name: &str) -> usize {
        // Exact matches win over wildcards, as in KiCad.
        if let Some((_, c)) = self.patterns.iter().find(|(p, _)| p == net_name) {
            return *c;
        }
        self.patterns.iter().find(|(p, _)| glob(p, net_name)).map(|(_, c)| *c).unwrap_or(0)
    }

    pub fn class(&self, i: usize) -> &NetClass {
        &self.classes[i]
    }
}

/// `*` and `?` wildcard match.
pub fn glob(pattern: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), s.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_kicad_wildcards() {
        assert!(glob("GND", "GND"));
        assert!(glob("/USB_*", "/USB_D+"));
        assert!(glob("Net-(C?-Pad1)", "Net-(C2-Pad1)"));
        assert!(!glob("Net-(C?-Pad1)", "Net-(C15-Pad1)"));
        assert!(glob("*", ""));
        assert!(!glob("VCC", "VCC2"));
    }

    #[test]
    fn parses_classes_and_patterns() {
        let pro = r#"{"board":{"design_settings":{"rules":{"min_clearance":0.1,"min_copper_edge_clearance":0.5}}},
            "net_settings":{"classes":[{"name":"Power","track_width":0.5},{"name":"Default","clearance":0.15,"track_width":0.2,"via_diameter":0.6,"via_drill":0.3}],
            "netclass_patterns":[{"netclass":"Power","pattern":"+*V"}]}}"#;
        let r = Rules::parse(pro).unwrap();
        assert_eq!(r.classes[0].name, "Default");
        assert_eq!(r.min_copper_edge_clearance, 0.5);
        let c = r.class_of("+5V");
        assert_eq!(r.class(c).name, "Power");
        assert_eq!(r.class(c).track_width, 0.5);
        assert_eq!(r.class_of("SDA"), 0);
    }
}
