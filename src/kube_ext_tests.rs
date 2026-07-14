// Unit tests for src/kube_ext.rs
//
// To integrate, append this to the bottom of src/kube_ext.rs:
//
//     #[cfg(test)]
//     #[path = "kube_ext_tests.rs"]
//     mod tests;
//
// or move the module inline. All tests build mock K8s objects using the
// k8s-openapi types and Default::default() — no cluster access required.

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStatus};
    use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, CronJobStatus};
    use k8s_openapi::api::core::v1::{
        Container, ContainerState, ContainerStateRunning, ContainerStateTerminated,
        ContainerStateWaiting, ContainerStatus, EnvVar, ExecAction, HTTPGetAction, Node,
        NodeCondition, NodeStatus, NodeSystemInfo, Pod, PodCondition, PodSpec, PodStatus,
        PodTemplateSpec, Probe, ResourceRequirements, TCPSocketAction,
    };
    use k8s_openapi::api::networking::v1::{
        HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressLoadBalancerIngress,
        IngressLoadBalancerStatus, IngressRule, IngressServiceBackend, IngressSpec, IngressStatus,
        IngressTLS, ServiceBackendPort,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use std::collections::BTreeMap;

    // ---- Helpers ----

    fn meta(name: &str, ns: &str) -> ObjectMeta {
        ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            ..Default::default()
        }
    }

    fn dep_with(
        replicas: i32,
        available: i32,
        updated: i32,
        ready: i32,
        conds: Vec<k8s_openapi::api::apps::v1::DeploymentCondition>,
    ) -> Deployment {
        Deployment {
            metadata: meta("app", "default"),
            spec: Some(DeploymentSpec {
                replicas: Some(replicas),
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        containers: vec![Container {
                            name: "app".to_string(),
                            image: Some("nginx:1.25".to_string()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            status: Some(DeploymentStatus {
                available_replicas: Some(available),
                updated_replicas: Some(updated),
                ready_replicas: Some(ready),
                conditions: Some(conds),
                ..Default::default()
            }),
        }
    }

    fn dep_condition(
        type_: &str,
        status: &str,
        reason: Option<&str>,
    ) -> k8s_openapi::api::apps::v1::DeploymentCondition {
        k8s_openapi::api::apps::v1::DeploymentCondition {
            type_: type_.to_string(),
            status: status.to_string(),
            reason: reason.map(String::from),
            message: None,
            last_transition_time: None,
            last_update_time: None,
        }
    }

    // ---- deployment_phase ----

    #[test]
    fn deployment_phase_no_status_is_progressing() {
        let dep = Deployment {
            metadata: meta("app", "default"),
            spec: None,
            status: None,
        };
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Progressing));
    }

    #[test]
    fn deployment_phase_available_when_new_replicaset_available() {
        let conds = vec![dep_condition(
            "Progressing",
            "True",
            Some("NewReplicaSetAvailable"),
        )];
        let dep = dep_with(3, 3, 3, 3, conds);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Available));
    }

    #[test]
    fn deployment_phase_progressing_when_reason_is_other() {
        let conds = vec![dep_condition("Progressing", "True", Some("ReplicaSetUpdated"))];
        let dep = dep_with(3, 1, 1, 1, conds);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Progressing));
    }

    #[test]
    fn deployment_phase_failed_when_replicafailure_true() {
        let conds = vec![dep_condition("ReplicaFailure", "True", Some("FailedCreate"))];
        let dep = dep_with(3, 0, 0, 0, conds);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Failed));
    }

    #[test]
    fn deployment_phase_failed_when_available_false() {
        let conds = vec![dep_condition("Available", "False", None)];
        let dep = dep_with(3, 0, 0, 0, conds);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Failed));
    }

    #[test]
    fn deployment_phase_available_when_desired_zero() {
        let dep = dep_with(0, 0, 0, 0, vec![]);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Available));
    }

    #[test]
    fn deployment_phase_degraded_when_some_available() {
        let dep = dep_with(3, 1, 1, 1, vec![]);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Degraded));
    }

    #[test]
    fn deployment_phase_progressing_when_none_available() {
        let dep = dep_with(3, 0, 0, 0, vec![]);
        assert!(matches!(deployment_phase(&dep), DeploymentPhase::Progressing));
    }

    // ---- replica_counts ----

    #[test]
    fn replica_counts_defaults_to_one_when_spec_missing_replicas() {
        let mut dep = dep_with(0, 0, 0, 0, vec![]);
        dep.spec.as_mut().unwrap().replicas = None;
        let counts = replica_counts(&dep);
        assert_eq!(counts.desired, 1);
    }

    #[test]
    fn replica_counts_reflects_status_fields() {
        let dep = dep_with(5, 4, 3, 2, vec![]);
        let counts = replica_counts(&dep);
        assert_eq!(counts.desired, 5);
        assert_eq!(counts.available, 4);
        assert_eq!(counts.updated, 3);
        assert_eq!(counts.ready, 2);
    }

    #[test]
    fn replica_counts_zero_when_status_missing() {
        let mut dep = dep_with(5, 0, 0, 0, vec![]);
        dep.status = None;
        let counts = replica_counts(&dep);
        assert_eq!(counts.desired, 5);
        assert_eq!(counts.available, 0);
    }

    // ---- primary_image ----

    #[test]
    fn primary_image_returns_first_container_image() {
        let dep = dep_with(1, 1, 1, 1, vec![]);
        assert_eq!(primary_image(&dep), "nginx:1.25");
    }

    #[test]
    fn primary_image_empty_when_no_containers() {
        let dep = Deployment {
            metadata: meta("app", "default"),
            spec: Some(DeploymentSpec {
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        containers: vec![],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            status: None,
        };
        assert_eq!(primary_image(&dep), "");
    }

    // ---- deployment_summary ----

    #[test]
    fn deployment_summary_populates_all_fields() {
        let mut dep = dep_with(2, 2, 2, 2, vec![]);
        let mut labels = BTreeMap::new();
        labels.insert("app".to_string(), "app".to_string());
        dep.metadata.labels = Some(labels.clone());
        let sum = deployment_summary(&dep);
        assert_eq!(sum.name, "app");
        assert_eq!(sum.namespace, "default");
        assert_eq!(sum.image, "nginx:1.25");
        assert_eq!(sum.replicas.desired, 2);
        assert!(matches!(sum.status, DeploymentPhase::Available));
        assert_eq!(sum.labels, labels);
    }

    // ---- deployment_detail ----

    #[test]
    fn deployment_detail_captures_env_command_args() {
        let mut dep = dep_with(1, 1, 1, 1, vec![]);
        let container = &mut dep
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .containers[0];
        container.env = Some(vec![
            EnvVar {
                name: "FOO".to_string(),
                value: Some("bar".to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "BAZ".to_string(),
                value: None,
                ..Default::default()
            },
        ]);
        container.command = Some(vec!["/bin/sh".to_string()]);
        container.args = Some(vec!["-c".to_string(), "echo".to_string()]);
        let detail = deployment_detail(&dep);
        assert_eq!(detail.env.len(), 2);
        assert_eq!(detail.env[0].name, "FOO");
        assert_eq!(detail.env[0].value.as_deref(), Some("bar"));
        assert_eq!(detail.env[1].value, None);
        assert_eq!(detail.command, vec!["/bin/sh".to_string()]);
        assert_eq!(detail.args, vec!["-c".to_string(), "echo".to_string()]);
    }

    #[test]
    fn deployment_detail_extracts_resource_limits_and_requests() {
        let mut dep = dep_with(1, 1, 1, 1, vec![]);
        let container = &mut dep
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .containers[0];
        let mut limits = BTreeMap::new();
        limits.insert("cpu".to_string(), Quantity("500m".to_string()));
        limits.insert("memory".to_string(), Quantity("512Mi".to_string()));
        let mut requests = BTreeMap::new();
        requests.insert("cpu".to_string(), Quantity("100m".to_string()));
        container.resources = Some(ResourceRequirements {
            limits: Some(limits),
            requests: Some(requests),
            ..Default::default()
        });
        let detail = deployment_detail(&dep);
        let l = detail.resource_limits.expect("limits present");
        assert_eq!(l.cpu.as_deref(), Some("500m"));
        assert_eq!(l.memory.as_deref(), Some("512Mi"));
        let r = detail.resource_requests.expect("requests present");
        assert_eq!(r.cpu.as_deref(), Some("100m"));
        assert_eq!(r.memory, None);
    }

    #[test]
    fn deployment_detail_extracts_probes() {
        let mut dep = dep_with(1, 1, 1, 1, vec![]);
        let container = &mut dep
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .containers[0];
        container.liveness_probe = Some(Probe {
            http_get: Some(HTTPGetAction {
                path: Some("/healthz".to_string()),
                port: IntOrString::Int(8080),
                ..Default::default()
            }),
            initial_delay_seconds: Some(10),
            period_seconds: Some(5),
            ..Default::default()
        });
        container.readiness_probe = Some(Probe {
            tcp_socket: Some(TCPSocketAction {
                port: IntOrString::Int(9090),
                ..Default::default()
            }),
            ..Default::default()
        });
        container.startup_probe = Some(Probe {
            exec: Some(ExecAction {
                command: Some(vec!["cat".to_string(), "/tmp/ok".to_string()]),
            }),
            ..Default::default()
        });
        let detail = deployment_detail(&dep);
        let liveness = detail.liveness_probe.expect("liveness present");
        assert_eq!(liveness.probe_type, "httpGet");
        assert_eq!(liveness.path.as_deref(), Some("/healthz"));
        assert_eq!(liveness.port, Some(8080));
        assert_eq!(liveness.initial_delay_seconds, Some(10));

        let readiness = detail.readiness_probe.expect("readiness present");
        assert_eq!(readiness.probe_type, "tcpSocket");
        assert_eq!(readiness.port, Some(9090));

        let startup = detail.startup_probe.expect("startup present");
        assert_eq!(startup.probe_type, "exec");
        assert_eq!(
            startup.command.as_deref(),
            Some(&["cat".to_string(), "/tmp/ok".to_string()][..])
        );
    }

    #[test]
    fn deployment_detail_probe_string_port_yields_none() {
        // Probes that use a named port should not panic and should report port as None.
        let mut dep = dep_with(1, 1, 1, 1, vec![]);
        let container = &mut dep
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap()
            .containers[0];
        container.readiness_probe = Some(Probe {
            http_get: Some(HTTPGetAction {
                path: Some("/".to_string()),
                port: IntOrString::String("http".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let detail = deployment_detail(&dep);
        assert_eq!(detail.readiness_probe.unwrap().port, None);
    }

    #[test]
    fn deployment_detail_captures_conditions() {
        let dep = dep_with(
            1,
            1,
            1,
            1,
            vec![
                dep_condition("Available", "True", Some("MinimumReplicasAvailable")),
                dep_condition("Progressing", "True", Some("NewReplicaSetAvailable")),
            ],
        );
        let detail = deployment_detail(&dep);
        assert_eq!(detail.conditions.len(), 2);
        assert_eq!(detail.conditions[0].condition_type, "Available");
        assert_eq!(detail.conditions[1].reason.as_deref(), Some("NewReplicaSetAvailable"));
    }

    // ---- pod_summary ----

    fn container_status(name: &str, ready: bool, restart: i32, state: ContainerState) -> ContainerStatus {
        ContainerStatus {
            name: name.to_string(),
            ready,
            restart_count: restart,
            state: Some(state),
            image: "nginx:1.25".to_string(),
            image_id: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn pod_summary_ready_when_all_containers_ready() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: Some(PodSpec { node_name: Some("node-a".to_string()), ..Default::default() }),
            status: Some(PodStatus {
                phase: Some("Running".to_string()),
                container_statuses: Some(vec![
                    container_status("c1", true, 0, ContainerState { running: Some(ContainerStateRunning { started_at: None }), ..Default::default() }),
                    container_status("c2", true, 1, ContainerState { running: Some(ContainerStateRunning { started_at: None }), ..Default::default() }),
                ]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert_eq!(sum.name, "p1");
        assert_eq!(sum.phase, "Running");
        assert!(sum.ready);
        assert_eq!(sum.restart_count, 1);
        assert_eq!(sum.node.as_deref(), Some("node-a"));
        assert_eq!(sum.container_statuses.len(), 2);
        assert_eq!(sum.container_statuses[0].state, "running");
    }

    #[test]
    fn pod_summary_not_ready_when_any_container_not_ready() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Running".to_string()),
                container_statuses: Some(vec![
                    container_status("c1", true, 0, ContainerState { running: Some(ContainerStateRunning { started_at: None }), ..Default::default() }),
                    container_status("c2", false, 3, ContainerState { waiting: Some(ContainerStateWaiting { reason: Some("CrashLoopBackOff".to_string()), message: None }), ..Default::default() }),
                ]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert!(!sum.ready);
        assert_eq!(sum.restart_count, 3);
        assert_eq!(sum.container_statuses[1].state, "waiting");
        assert_eq!(sum.container_statuses[1].state_reason.as_deref(), Some("CrashLoopBackOff"));
    }

    #[test]
    fn pod_summary_terminated_container_state() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Failed".to_string()),
                container_statuses: Some(vec![container_status("c1", false, 0, ContainerState {
                    terminated: Some(ContainerStateTerminated {
                        reason: Some("Error".to_string()),
                        exit_code: 1,
                        ..Default::default()
                    }),
                    ..Default::default()
                })]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert_eq!(sum.container_statuses[0].state, "terminated");
        assert_eq!(sum.container_statuses[0].state_reason.as_deref(), Some("Error"));
    }

    #[test]
    fn pod_summary_not_ready_when_no_containers() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Pending".to_string()),
                container_statuses: None,
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert!(!sum.ready);
        assert_eq!(sum.container_statuses.len(), 0);
    }

    #[test]
    fn pod_summary_phase_defaults_to_unknown() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: None,
        };
        let sum = pod_summary(&pod);
        assert_eq!(sum.phase, "Unknown");
    }

    #[test]
    fn pod_summary_oom_killed_current_state() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Failed".to_string()),
                container_statuses: Some(vec![container_status(
                    "c1",
                    false,
                    0,
                    ContainerState {
                        terminated: Some(ContainerStateTerminated {
                            reason: Some("OOMKilled".to_string()),
                            exit_code: 137,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert!(sum.oom_killed);
        assert!(sum.container_statuses[0].oom_killed);
    }

    #[test]
    fn pod_summary_oom_killed_last_state_after_restart() {
        // Container restarted after OOM: current state is running, but
        // last_state carries the OOMKilled signal.
        let mut cs = container_status(
            "c1",
            true,
            2,
            ContainerState {
                running: Some(ContainerStateRunning { started_at: None }),
                ..Default::default()
            },
        );
        cs.last_state = Some(ContainerState {
            terminated: Some(ContainerStateTerminated {
                reason: Some("OOMKilled".to_string()),
                exit_code: 137,
                ..Default::default()
            }),
            ..Default::default()
        });
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Running".to_string()),
                container_statuses: Some(vec![cs]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert!(sum.oom_killed, "OOMKilled must surface from last_state");
        assert!(sum.container_statuses[0].oom_killed);
    }

    #[test]
    fn pod_summary_oom_false_when_terminated_other_reason() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Failed".to_string()),
                container_statuses: Some(vec![container_status(
                    "c1",
                    false,
                    0,
                    ContainerState {
                        terminated: Some(ContainerStateTerminated {
                            reason: Some("Error".to_string()),
                            exit_code: 1,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert!(!sum.oom_killed);
        assert!(!sum.container_statuses[0].oom_killed);
    }

    #[test]
    fn pod_summary_conditions_mapped() {
        let pod = Pod {
            metadata: meta("p1", "default"),
            spec: None,
            status: Some(PodStatus {
                phase: Some("Running".to_string()),
                conditions: Some(vec![PodCondition {
                    type_: "Ready".to_string(),
                    status: "True".to_string(),
                    reason: Some("OK".to_string()),
                    message: Some("all good".to_string()),
                    last_probe_time: None,
                    last_transition_time: None,
                }]),
                ..Default::default()
            }),
        };
        let sum = pod_summary(&pod);
        assert_eq!(sum.conditions.len(), 1);
        assert!(sum.conditions[0].status);
        assert_eq!(sum.conditions[0].condition_type, "Ready");
    }

    // ---- ingress_summary / ingress_detail ----

    fn make_ingress() -> Ingress {
        Ingress {
            metadata: meta("web", "default"),
            spec: Some(IngressSpec {
                ingress_class_name: Some("nginx".to_string()),
                rules: Some(vec![IngressRule {
                    host: Some("example.com".to_string()),
                    http: Some(HTTPIngressRuleValue {
                        paths: vec![HTTPIngressPath {
                            path: Some("/api".to_string()),
                            path_type: "Prefix".to_string(),
                            backend: IngressBackend {
                                service: Some(IngressServiceBackend {
                                    name: "web".to_string(),
                                    port: Some(ServiceBackendPort {
                                        number: Some(80),
                                        ..Default::default()
                                    }),
                                }),
                                ..Default::default()
                            },
                        }],
                    }),
                }]),
                tls: Some(vec![IngressTLS {
                    hosts: Some(vec!["example.com".to_string()]),
                    secret_name: Some("example-tls".to_string()),
                }]),
                ..Default::default()
            }),
            status: Some(IngressStatus {
                load_balancer: Some(IngressLoadBalancerStatus {
                    ingress: Some(vec![IngressLoadBalancerIngress {
                        ip: Some("10.0.0.1".to_string()),
                        hostname: Some("lb.example.com".to_string()),
                        ..Default::default()
                    }]),
                }),
            }),
        }
    }

    #[test]
    fn ingress_summary_extracts_hosts_and_addresses() {
        let ing = make_ingress();
        let s = ingress_summary(&ing);
        assert_eq!(s.name, "web");
        assert_eq!(s.hosts, vec!["example.com".to_string()]);
        assert_eq!(s.ingress_class.as_deref(), Some("nginx"));
        assert_eq!(s.addresses, vec!["lb.example.com".to_string()]); // hostname preferred over ip
    }

    #[test]
    fn ingress_summary_addresses_fall_back_to_ip() {
        let mut ing = make_ingress();
        ing.status
            .as_mut()
            .unwrap()
            .load_balancer
            .as_mut()
            .unwrap()
            .ingress
            .as_mut()
            .unwrap()[0]
            .hostname = None;
        let s = ingress_summary(&ing);
        assert_eq!(s.addresses, vec!["10.0.0.1".to_string()]);
    }

    #[test]
    fn ingress_detail_extracts_rules_and_tls() {
        let ing = make_ingress();
        let d = ingress_detail(&ing);
        assert_eq!(d.rules.len(), 1);
        assert_eq!(d.rules[0].host.as_deref(), Some("example.com"));
        assert_eq!(d.rules[0].paths[0].path, "/api");
        assert_eq!(d.rules[0].paths[0].path_type, "Prefix");
        assert_eq!(d.rules[0].paths[0].service_name, "web");
        assert_eq!(d.rules[0].paths[0].service_port, 80);
        assert_eq!(d.tls.len(), 1);
        assert_eq!(d.tls[0].secret_name.as_deref(), Some("example-tls"));
    }

    #[test]
    fn ingress_path_defaults_when_missing() {
        let mut ing = make_ingress();
        let path = &mut ing
            .spec
            .as_mut()
            .unwrap()
            .rules
            .as_mut()
            .unwrap()[0]
            .http
            .as_mut()
            .unwrap()
            .paths[0];
        path.path = None;
        // Also drop the backend service to exercise the default fallback (port 80).
        path.backend.service = None;
        let d = ingress_detail(&ing);
        assert_eq!(d.rules[0].paths[0].path, "/");
        assert_eq!(d.rules[0].paths[0].service_port, 80);
        assert_eq!(d.rules[0].paths[0].service_name, "");
    }

    // ---- cronjob_summary ----

    #[test]
    fn cronjob_summary_basic() {
        let cj = CronJob {
            metadata: meta("nightly", "default"),
            spec: Some(CronJobSpec {
                schedule: "0 0 * * *".to_string(),
                suspend: Some(true),
                ..Default::default()
            }),
            status: Some(CronJobStatus {
                active: Some(vec![Default::default(), Default::default()]),
                last_schedule_time: None,
                ..Default::default()
            }),
        };
        let s = cronjob_summary(&cj);
        assert_eq!(s.name, "nightly");
        assert_eq!(s.schedule, "0 0 * * *");
        assert!(s.suspend);
        assert_eq!(s.active_count, 2);
    }

    #[test]
    fn cronjob_summary_defaults_when_empty() {
        let cj = CronJob {
            metadata: meta("cj", "default"),
            spec: None,
            status: None,
        };
        let s = cronjob_summary(&cj);
        assert_eq!(s.schedule, "");
        assert!(!s.suspend);
        assert_eq!(s.active_count, 0);
    }

    // ---- node_summary ----

    fn node_with(status: &str, roles: Vec<(&str, &str)>) -> Node {
        let mut labels = BTreeMap::new();
        for (k, v) in roles {
            labels.insert(k.to_string(), v.to_string());
        }
        Node {
            metadata: ObjectMeta {
                name: Some("n1".to_string()),
                labels: if labels.is_empty() { None } else { Some(labels) },
                ..Default::default()
            },
            spec: None,
            status: Some(NodeStatus {
                conditions: Some(vec![NodeCondition {
                    type_: "Ready".to_string(),
                    status: status.to_string(),
                    reason: None,
                    message: None,
                    last_transition_time: None,
                    last_heartbeat_time: None,
                }]),
                node_info: Some(NodeSystemInfo {
                    os_image: "Ubuntu 22.04".to_string(),
                    kernel_version: "5.15".to_string(),
                    kubelet_version: "v1.32.0".to_string(),
                    ..Default::default()
                }),
                capacity: Some({
                    let mut m = BTreeMap::new();
                    m.insert("cpu".to_string(), Quantity("8".to_string()));
                    m.insert("memory".to_string(), Quantity("32Gi".to_string()));
                    m
                }),
                allocatable: Some({
                    let mut m = BTreeMap::new();
                    m.insert("cpu".to_string(), Quantity("7".to_string()));
                    m.insert("memory".to_string(), Quantity("30Gi".to_string()));
                    m
                }),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn node_summary_ready_status() {
        let n = node_with("True", vec![("node-role.kubernetes.io/worker", "")]);
        let s = node_summary(&n);
        assert_eq!(s.status, "Ready");
        assert_eq!(s.roles, vec!["worker".to_string()]);
        assert_eq!(s.cpu_capacity.as_deref(), Some("8"));
        assert_eq!(s.memory_capacity.as_deref(), Some("32Gi"));
        assert_eq!(s.cpu_allocatable.as_deref(), Some("7"));
        assert_eq!(s.os_image.as_deref(), Some("Ubuntu 22.04"));
        assert_eq!(s.kubelet_version.as_deref(), Some("v1.32.0"));
    }

    #[test]
    fn node_summary_not_ready_status() {
        let n = node_with("False", vec![]);
        let s = node_summary(&n);
        assert_eq!(s.status, "NotReady");
        // Falls back to <none> when no role labels are present.
        assert_eq!(s.roles, vec!["<none>".to_string()]);
    }

    #[test]
    fn node_summary_unknown_status_when_no_ready_condition() {
        let node = Node {
            metadata: ObjectMeta {
                name: Some("n1".to_string()),
                labels: None,
                ..Default::default()
            },
            spec: None,
            status: Some(NodeStatus {
                conditions: Some(vec![NodeCondition {
                    type_: "DiskPressure".to_string(),
                    status: "False".to_string(),
                    reason: None,
                    message: None,
                    last_transition_time: None,
                    last_heartbeat_time: None,
                }]),
                ..Default::default()
            }),
        };
        let s = node_summary(&node);
        assert_eq!(s.status, "Unknown");
    }

    #[test]
    fn node_summary_multiple_roles_sorted() {
        let n = node_with(
            "True",
            vec![
                ("node-role.kubernetes.io/control-plane", ""),
                ("node-role.kubernetes.io/master", ""),
            ],
        );
        let s = node_summary(&n);
        assert_eq!(
            s.roles,
            vec!["control-plane".to_string(), "master".to_string()]
        );
    }

    #[test]
    fn node_summary_uses_kubernetes_io_role_when_no_prefixed_label() {
        let n = node_with("True", vec![("kubernetes.io/role", "gpu")]);
        let s = node_summary(&n);
        assert_eq!(s.roles, vec!["gpu".to_string()]);
    }

    // Silence unused warning if Condition import isn't strictly needed.
    #[allow(dead_code)]
    fn _condition_used(_: Condition) {}
}
