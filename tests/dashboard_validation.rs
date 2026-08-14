use std::collections::HashSet;
use std::fs;

fn load_dashboard(name: &str) -> serde_json::Value {
    let path = format!("dashboards/{name}");
    let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn validate_dashboard(name: &str) {
    let dash = load_dashboard(name);

    assert!(dash["title"].is_string(), "{name}: missing title");
    assert!(dash["uid"].is_string(), "{name}: missing uid");
    assert!(dash["panels"].is_array(), "{name}: missing panels array");

    let panels = dash["panels"].as_array().unwrap();
    assert!(!panels.is_empty(), "{name}: no panels defined");

    let mut ids = HashSet::new();
    for panel in panels {
        let id = panel["id"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: panel missing numeric id: {panel}"));
        assert!(ids.insert(id), "{name}: duplicate panel id {id}");

        assert!(panel["type"].is_string(), "{name}: panel {id} missing type");

        if panel["type"].as_str() == Some("row") {
            continue;
        }

        assert!(
            panel["gridPos"].is_object(),
            "{name}: panel {id} missing gridPos"
        );

        let targets = panel.get("targets");
        assert!(
            targets.is_some() && targets.unwrap().is_array(),
            "{name}: panel {id} ({}) missing targets array",
            panel["title"].as_str().unwrap_or("untitled")
        );

        let targets = targets.unwrap().as_array().unwrap();
        assert!(
            !targets.is_empty(),
            "{name}: panel {id} ({}) has empty targets",
            panel["title"].as_str().unwrap_or("untitled")
        );

        for target in targets {
            assert!(
                target["expr"].is_string(),
                "{name}: panel {id} target missing expr"
            );
            let expr = target["expr"].as_str().unwrap();
            assert!(!expr.is_empty(), "{name}: panel {id} target has empty expr");
        }
    }

    let templating = &dash["templating"]["list"];
    assert!(templating.is_array(), "{name}: missing templating.list");

    let vars = templating.as_array().unwrap();
    let has_datasource = vars
        .iter()
        .any(|v| v["name"].as_str() == Some("datasource"));
    assert!(
        has_datasource,
        "{name}: missing datasource template variable"
    );

    assert!(
        dash["schemaVersion"].is_u64(),
        "{name}: missing schemaVersion"
    );
}

#[test]
fn dashboard_overview_is_valid() {
    validate_dashboard("deckwatch-overview.json");
}

#[test]
fn dashboard_mcp_is_valid() {
    validate_dashboard("deckwatch-mcp.json");
}

#[test]
fn dashboard_infrastructure_is_valid() {
    validate_dashboard("deckwatch-infrastructure.json");
}

#[test]
fn dashboard_infrastructure_has_plugin_panels() {
    let dash = load_dashboard("deckwatch-infrastructure.json");
    let panels = dash["panels"].as_array().unwrap();

    let has_plugins_row = panels
        .iter()
        .any(|p| p["type"].as_str() == Some("row") && p["title"].as_str() == Some("Plugins"));
    assert!(
        has_plugins_row,
        "infrastructure dashboard must have a Plugins row"
    );

    let plugin_metric_panels: Vec<_> = panels
        .iter()
        .filter(|p| {
            p["type"].as_str() != Some("row")
                && p.get("targets")
                    .and_then(|t| t.as_array())
                    .map(|targets| {
                        targets.iter().any(|t| {
                            t["expr"]
                                .as_str()
                                .map(|e| e.contains("deckwatch_plugin"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
        })
        .collect();

    assert!(
        plugin_metric_panels.len() >= 6,
        "expected at least 6 plugin metric panels, found {}",
        plugin_metric_panels.len()
    );
}

#[test]
fn dashboard_infrastructure_has_psi_panels() {
    let dash = load_dashboard("deckwatch-infrastructure.json");
    let panels = dash["panels"].as_array().unwrap();

    let psi_panels: Vec<_> = panels
        .iter()
        .filter(|p| {
            p.get("targets")
                .and_then(|t| t.as_array())
                .map(|targets| {
                    targets.iter().any(|t| {
                        t["expr"]
                            .as_str()
                            .map(|e| e.contains("container_pressure"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .collect();

    assert_eq!(
        psi_panels.len(),
        3,
        "expected 3 PSI pressure panels (CPU, Memory, I/O), found {}",
        psi_panels.len()
    );
}

#[test]
fn all_dashboards_cross_link_via_tag() {
    for name in &[
        "deckwatch-overview.json",
        "deckwatch-mcp.json",
        "deckwatch-infrastructure.json",
    ] {
        let dash = load_dashboard(name);
        let tags = dash["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: missing tags"));
        let has_deckwatch_tag = tags.iter().any(|t| t.as_str() == Some("deckwatch"));
        assert!(
            has_deckwatch_tag,
            "{name}: missing 'deckwatch' tag for cross-linking"
        );
    }
}
