// Unit tests for the pure helpers in src/handlers/deployments.rs
// (build_probe, build_resources, resolve_ports).

use super::*;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

// ---- build_probe ----

#[test]
fn build_probe_http_get_uses_provided_port_and_path() {
    let input = ProbeInput {
        probe_type: "httpGet".to_string(),
        path: Some("/healthz".to_string()),
        port: Some(8080),
        command: None,
        initial_delay_seconds: Some(5),
        period_seconds: Some(10),
        timeout_seconds: Some(2),
        failure_threshold: Some(3),
        success_threshold: Some(1),
    };
    let probe = build_probe(input);
    let http = probe.http_get.expect("httpGet action present");
    assert_eq!(http.path.as_deref(), Some("/healthz"));
    assert!(matches!(http.port, IntOrString::Int(8080)));
    assert_eq!(probe.initial_delay_seconds, Some(5));
    assert_eq!(probe.period_seconds, Some(10));
    assert_eq!(probe.timeout_seconds, Some(2));
    assert_eq!(probe.failure_threshold, Some(3));
    assert_eq!(probe.success_threshold, Some(1));
    assert!(probe.tcp_socket.is_none());
    assert!(probe.exec.is_none());
}

#[test]
fn build_probe_http_get_defaults_port_to_80_when_omitted() {
    let input = ProbeInput {
        probe_type: "httpGet".to_string(),
        path: Some("/".to_string()),
        port: None,
        command: None,
        initial_delay_seconds: None,
        period_seconds: None,
        timeout_seconds: None,
        failure_threshold: None,
        success_threshold: None,
    };
    let probe = build_probe(input);
    let http = probe.http_get.unwrap();
    assert!(matches!(http.port, IntOrString::Int(80)));
}

#[test]
fn build_probe_tcp_socket_variant() {
    let input = ProbeInput {
        probe_type: "tcpSocket".to_string(),
        path: None,
        port: Some(9090),
        command: None,
        initial_delay_seconds: None,
        period_seconds: None,
        timeout_seconds: None,
        failure_threshold: None,
        success_threshold: None,
    };
    let probe = build_probe(input);
    assert!(probe.http_get.is_none());
    let tcp = probe.tcp_socket.expect("tcpSocket action present");
    assert!(matches!(tcp.port, IntOrString::Int(9090)));
}

#[test]
fn build_probe_exec_variant() {
    let input = ProbeInput {
        probe_type: "exec".to_string(),
        path: None,
        port: None,
        command: Some(vec!["cat".to_string(), "/tmp/ready".to_string()]),
        initial_delay_seconds: None,
        period_seconds: None,
        timeout_seconds: None,
        failure_threshold: None,
        success_threshold: None,
    };
    let probe = build_probe(input);
    assert!(probe.http_get.is_none());
    assert!(probe.tcp_socket.is_none());
    let exec = probe.exec.expect("exec action present");
    assert_eq!(
        exec.command.as_deref(),
        Some(&["cat".to_string(), "/tmp/ready".to_string()][..])
    );
}

#[test]
fn build_probe_unknown_type_yields_empty_probe() {
    let input = ProbeInput {
        probe_type: "gibberish".to_string(),
        path: None,
        port: None,
        command: None,
        initial_delay_seconds: Some(1),
        period_seconds: None,
        timeout_seconds: None,
        failure_threshold: None,
        success_threshold: None,
    };
    let probe = build_probe(input);
    assert!(probe.http_get.is_none());
    assert!(probe.tcp_socket.is_none());
    assert!(probe.exec.is_none());
    // Numeric fields still flow through even for unknown types.
    assert_eq!(probe.initial_delay_seconds, Some(1));
}

// ---- build_resources ----

#[test]
fn build_resources_returns_none_when_both_missing() {
    assert!(build_resources(None, None).is_none());
}

#[test]
fn build_resources_only_requests() {
    let r = build_resources(
        Some(ResourceSpec {
            cpu: Some("100m".to_string()),
            memory: Some("128Mi".to_string()),
        }),
        None,
    )
    .unwrap();
    let requests = r.requests.unwrap();
    assert_eq!(requests.get("cpu").unwrap().0, "100m");
    assert_eq!(requests.get("memory").unwrap().0, "128Mi");
    assert!(r.limits.is_none());
}

#[test]
fn build_resources_only_limits() {
    let r = build_resources(
        None,
        Some(ResourceSpec {
            cpu: Some("500m".to_string()),
            memory: None,
        }),
    )
    .unwrap();
    let limits = r.limits.unwrap();
    assert_eq!(limits.get("cpu").unwrap().0, "500m");
    assert!(limits.get("memory").is_none());
    assert!(r.requests.is_none());
}

#[test]
fn build_resources_both() {
    let r = build_resources(
        Some(ResourceSpec {
            cpu: Some("100m".to_string()),
            memory: Some("128Mi".to_string()),
        }),
        Some(ResourceSpec {
            cpu: Some("500m".to_string()),
            memory: Some("512Mi".to_string()),
        }),
    )
    .unwrap();
    assert_eq!(r.requests.as_ref().unwrap().get("cpu").unwrap().0, "100m");
    assert_eq!(r.limits.as_ref().unwrap().get("cpu").unwrap().0, "500m");
}

// ---- resolve_ports ----

#[test]
fn resolve_ports_returns_none_when_both_inputs_missing() {
    assert!(resolve_ports(None, None).is_none());
}

#[test]
fn resolve_ports_legacy_single_port_is_wrapped_into_one_entry() {
    let ports = resolve_ports(Some(8080), None).expect("some ports");
    assert_eq!(ports.len(), 1);
    assert_eq!(ports[0].container_port, 8080);
    assert!(ports[0].name.is_none());
    assert!(ports[0].protocol.is_none());
}

#[test]
fn resolve_ports_new_array_wins_over_legacy_field() {
    let list = vec![
        PortInput {
            port: 9000,
            name: Some("api".to_string()),
            protocol: Some("tcp".to_string()),
        },
        PortInput {
            port: 9001,
            name: Some("metrics".to_string()),
            protocol: Some("TCP".to_string()),
        },
    ];
    // Legacy `port` is 80 but should be ignored because `ports` is set.
    let ports = resolve_ports(Some(80), Some(list)).expect("some ports");
    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0].container_port, 9000);
    assert_eq!(ports[0].name.as_deref(), Some("api"));
    // Protocol was `"tcp"` — must be normalized to uppercase for k8s.
    assert_eq!(ports[0].protocol.as_deref(), Some("TCP"));
    assert_eq!(ports[1].container_port, 9001);
    assert_eq!(ports[1].name.as_deref(), Some("metrics"));
    assert_eq!(ports[1].protocol.as_deref(), Some("TCP"));
}

#[test]
fn resolve_ports_empty_array_yields_none() {
    let ports = resolve_ports(None, Some(vec![]));
    assert!(ports.is_none());
}

#[test]
fn resolve_ports_empty_name_and_protocol_strings_are_dropped() {
    let list = vec![PortInput {
        port: 8080,
        name: Some("".to_string()),
        protocol: Some("".to_string()),
    }];
    let ports = resolve_ports(None, Some(list)).expect("some ports");
    assert_eq!(ports.len(), 1);
    assert!(ports[0].name.is_none());
    assert!(ports[0].protocol.is_none());
}

#[test]
fn resolve_ports_udp_protocol_is_upper_cased() {
    let list = vec![PortInput {
        port: 53,
        name: Some("dns".to_string()),
        protocol: Some("udp".to_string()),
    }];
    let ports = resolve_ports(None, Some(list)).expect("some ports");
    assert_eq!(ports[0].protocol.as_deref(), Some("UDP"));
}
