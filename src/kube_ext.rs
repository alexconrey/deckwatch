use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::CronJob;
use k8s_openapi::api::core::v1::{ContainerStatus, Node, Pod, Probe};
use k8s_openapi::api::networking::v1::Ingress;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentPhase {
    Available,
    Progressing,
    Degraded,
    Failed,
}

#[derive(Serialize, Debug)]
pub struct ReplicaCounts {
    pub desired: i32,
    pub ready: i32,
    pub available: i32,
    pub updated: i32,
}

#[derive(Serialize, Debug)]
pub struct DeploymentConditionSummary {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct DeploymentSummary {
    pub name: String,
    pub namespace: String,
    pub image: String,
    pub replicas: ReplicaCounts,
    pub status: DeploymentPhase,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    /// Included in the summary so the deployments table can render a cost
    /// chip without a follow-up detail fetch per row. `None` when the spec
    /// has no resources.requests block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_requests: Option<ResourceSpecOutput>,
}

#[derive(Serialize, Debug)]
pub struct DeploymentDetail {
    pub name: String,
    pub namespace: String,
    pub image: String,
    pub replicas: ReplicaCounts,
    pub status: DeploymentPhase,
    pub conditions: Vec<DeploymentConditionSummary>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub env: Vec<EnvVarOutput>,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub resource_limits: Option<ResourceSpecOutput>,
    pub resource_requests: Option<ResourceSpecOutput>,
    pub liveness_probe: Option<ProbeOutput>,
    pub readiness_probe: Option<ProbeOutput>,
    pub startup_probe: Option<ProbeOutput>,
}

#[derive(Serialize, Debug)]
pub struct EnvVarOutput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct ResourceSpecOutput {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProbeOutput {
    pub probe_type: String,
    pub path: Option<String>,
    pub port: Option<i32>,
    pub command: Option<Vec<String>>,
    pub initial_delay_seconds: Option<i32>,
    pub period_seconds: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub failure_threshold: Option<i32>,
    pub success_threshold: Option<i32>,
}

#[derive(Serialize, Debug)]
pub struct PodConditionSummary {
    pub condition_type: String,
    pub status: bool,
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct PodSummary {
    pub name: String,
    pub phase: String,
    pub ready: bool,
    pub restart_count: i32,
    pub node: Option<String>,
    pub started_at: Option<String>,
    pub conditions: Vec<PodConditionSummary>,
    pub container_statuses: Vec<ContainerStatusSummary>,
    /// True if any container is currently OOMKilled (terminated state) or was
    /// OOMKilled at its most recent termination (last_state) — the latter is
    /// what surfaces after the kubelet restarts a memory-crashed container.
    pub oom_killed: bool,
}

#[derive(Serialize, Debug)]
pub struct ContainerStatusSummary {
    pub name: String,
    pub ready: bool,
    pub restart_count: i32,
    pub state: String,
    pub state_reason: Option<String>,
    pub image: String,
    /// True when this specific container's current OR previous termination
    /// reason is "OOMKilled". After a restart the current state is `running`
    /// again — the OOM signal lives in `last_state.terminated`.
    pub oom_killed: bool,
}

pub fn deployment_phase(dep: &Deployment) -> DeploymentPhase {
    let status = match &dep.status {
        Some(s) => s,
        None => return DeploymentPhase::Progressing,
    };

    let desired = dep
        .spec
        .as_ref()
        .and_then(|s| s.replicas)
        .unwrap_or(1);
    let available = status.available_replicas.unwrap_or(0);
    let updated = status.updated_replicas.unwrap_or(0);

    if let Some(conditions) = &status.conditions {
        for c in conditions {
            if c.type_ == "ReplicaFailure" && c.status == "True" {
                return DeploymentPhase::Failed;
            }
            if c.type_ == "Available" && c.status == "False" {
                return DeploymentPhase::Failed;
            }
        }

        for c in conditions {
            if c.type_ == "Progressing" && c.status == "True" {
                if let Some(reason) = &c.reason {
                    if reason == "NewReplicaSetAvailable"
                        && available >= desired
                        && updated >= desired
                    {
                        return DeploymentPhase::Available;
                    }
                    return DeploymentPhase::Progressing;
                }
            }
        }
    }

    if desired == 0 {
        DeploymentPhase::Available
    } else if available >= desired {
        DeploymentPhase::Available
    } else if available > 0 {
        DeploymentPhase::Degraded
    } else {
        DeploymentPhase::Progressing
    }
}

pub fn replica_counts(dep: &Deployment) -> ReplicaCounts {
    let desired = dep
        .spec
        .as_ref()
        .and_then(|s| s.replicas)
        .unwrap_or(1);
    let status = dep.status.as_ref();
    ReplicaCounts {
        desired,
        ready: status.and_then(|s| s.ready_replicas).unwrap_or(0),
        available: status.and_then(|s| s.available_replicas).unwrap_or(0),
        updated: status.and_then(|s| s.updated_replicas).unwrap_or(0),
    }
}

pub fn primary_image(dep: &Deployment) -> String {
    dep.spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first())
        .map(|c| c.image.clone().unwrap_or_default())
        .unwrap_or_default()
}

pub fn deployment_summary(dep: &Deployment) -> DeploymentSummary {
    let meta = &dep.metadata;
    let resource_requests = dep
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first())
        .and_then(|c| c.resources.as_ref())
        .and_then(|r| r.requests.as_ref())
        .map(|l| ResourceSpecOutput {
            cpu: l.get("cpu").map(|q| q.0.clone()),
            memory: l.get("memory").map(|q| q.0.clone()),
        });
    DeploymentSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        image: primary_image(dep),
        replicas: replica_counts(dep),
        status: deployment_phase(dep),
        created_at: meta
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        resource_requests,
    }
}

pub fn deployment_detail(dep: &Deployment) -> DeploymentDetail {
    let meta = &dep.metadata;
    let container = dep
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|s| s.containers.first());

    let env = container
        .map(|c| {
            c.env
                .as_ref()
                .map(|vars| {
                    vars.iter()
                        .map(|v| EnvVarOutput {
                            name: v.name.clone(),
                            value: v.value.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let command = container
        .and_then(|c| c.command.clone())
        .unwrap_or_default();

    let args = container
        .and_then(|c| c.args.clone())
        .unwrap_or_default();

    let resources = container.and_then(|c| c.resources.as_ref());

    let resource_limits = resources.and_then(|r| {
        r.limits.as_ref().map(|l| ResourceSpecOutput {
            cpu: l.get("cpu").map(|q| q.0.clone()),
            memory: l.get("memory").map(|q| q.0.clone()),
        })
    });

    let resource_requests = resources.and_then(|r| {
        r.requests.as_ref().map(|l| ResourceSpecOutput {
            cpu: l.get("cpu").map(|q| q.0.clone()),
            memory: l.get("memory").map(|q| q.0.clone()),
        })
    });

    let conditions = dep
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map(|conds| {
            conds
                .iter()
                .map(|c| DeploymentConditionSummary {
                    condition_type: c.type_.clone(),
                    status: c.status.clone(),
                    reason: c.reason.clone(),
                    message: c.message.clone(),
                    last_transition: c.last_transition_time.as_ref().map(|t| t.0.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    DeploymentDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        image: primary_image(dep),
        replicas: replica_counts(dep),
        status: deployment_phase(dep),
        conditions,
        created_at: meta
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        annotations: meta.annotations.clone().unwrap_or_default(),
        env,
        command,
        args,
        resource_limits,
        resource_requests,
        liveness_probe: container.and_then(|c| c.liveness_probe.as_ref()).map(extract_probe),
        readiness_probe: container.and_then(|c| c.readiness_probe.as_ref()).map(extract_probe),
        startup_probe: container.and_then(|c| c.startup_probe.as_ref()).map(extract_probe),
    }
}

fn extract_probe(probe: &Probe) -> ProbeOutput {
    let (probe_type, path, port, command) = if let Some(http) = &probe.http_get {
        let port = match &http.port {
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(p) => Some(*p),
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(_) => None,
        };
        ("httpGet".to_string(), http.path.clone(), port, None)
    } else if let Some(tcp) = &probe.tcp_socket {
        let port = match &tcp.port {
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(p) => Some(*p),
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(_) => None,
        };
        ("tcpSocket".to_string(), None, port, None)
    } else if let Some(exec) = &probe.exec {
        ("exec".to_string(), None, None, exec.command.clone())
    } else {
        ("unknown".to_string(), None, None, None)
    };

    ProbeOutput {
        probe_type,
        path,
        port,
        command,
        initial_delay_seconds: probe.initial_delay_seconds,
        period_seconds: probe.period_seconds,
        timeout_seconds: probe.timeout_seconds,
        failure_threshold: probe.failure_threshold,
        success_threshold: probe.success_threshold,
    }
}

fn container_state_info(cs: &ContainerStatus) -> (String, Option<String>) {
    match &cs.state {
        Some(state) => {
            if let Some(running) = &state.running {
                ("running".to_string(), running.started_at.as_ref().map(|t| t.0.to_string()))
            } else if let Some(waiting) = &state.waiting {
                ("waiting".to_string(), waiting.reason.clone())
            } else if let Some(terminated) = &state.terminated {
                ("terminated".to_string(), terminated.reason.clone())
            } else {
                ("unknown".to_string(), None)
            }
        }
        None => ("unknown".to_string(), None),
    }
}

/// True if the container's current termination OR its most recent prior
/// termination was OOMKilled. Checking `last_state` is essential: after the
/// kubelet restarts the container, the current state flips back to `running`
/// and the OOM signal is only visible in `last_state.terminated.reason`.
fn container_oom_killed(cs: &ContainerStatus) -> bool {
    let is_oom = |st: &k8s_openapi::api::core::v1::ContainerState| {
        st.terminated
            .as_ref()
            .and_then(|t| t.reason.as_deref())
            == Some("OOMKilled")
    };
    cs.state.as_ref().is_some_and(is_oom)
        || cs.last_state.as_ref().is_some_and(is_oom)
}

pub fn pod_summary(pod: &Pod) -> PodSummary {
    let meta = &pod.metadata;
    let status = pod.status.as_ref();

    let phase = status
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let runtime_statuses = status.and_then(|s| s.container_statuses.as_ref());

    let mut container_statuses: Vec<ContainerStatusSummary> = Vec::new();
    if let Some(spec) = pod.spec.as_ref() {
        for c in &spec.containers {
            let cs = runtime_statuses.and_then(|list| list.iter().find(|s| s.name == c.name));
            if let Some(cs) = cs {
                let (state, state_reason) = container_state_info(cs);
                container_statuses.push(ContainerStatusSummary {
                    name: cs.name.clone(),
                    ready: cs.ready,
                    restart_count: cs.restart_count,
                    state,
                    state_reason,
                    image: cs.image.clone(),
                    oom_killed: container_oom_killed(cs),
                });
            } else {
                container_statuses.push(ContainerStatusSummary {
                    name: c.name.clone(),
                    ready: false,
                    restart_count: 0,
                    state: "pending".to_string(),
                    state_reason: Some("NotStartedYet".to_string()),
                    image: c.image.clone().unwrap_or_default(),
                    oom_killed: false,
                });
            }
        }
    }

    if let Some(list) = runtime_statuses {
        for cs in list {
            if container_statuses.iter().any(|existing| existing.name == cs.name) {
                continue;
            }
            let (state, state_reason) = container_state_info(cs);
            container_statuses.push(ContainerStatusSummary {
                name: cs.name.clone(),
                ready: cs.ready,
                restart_count: cs.restart_count,
                state,
                state_reason,
                image: cs.image.clone(),
                oom_killed: container_oom_killed(cs),
            });
        }
    }

    let ready = !container_statuses.is_empty()
        && container_statuses.iter().all(|cs| cs.ready);
    let restart_count: i32 = container_statuses.iter().map(|cs| cs.restart_count).sum();
    let oom_killed = container_statuses.iter().any(|cs| cs.oom_killed);

    let started_at = status
        .and_then(|s| s.start_time.as_ref())
        .map(|t| t.0.to_string());

    let conditions: Vec<PodConditionSummary> = status
        .and_then(|s| s.conditions.as_ref())
        .map(|conds| {
            conds
                .iter()
                .map(|c| PodConditionSummary {
                    condition_type: c.type_.clone(),
                    status: c.status == "True",
                    reason: c.reason.clone(),
                    message: c.message.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    PodSummary {
        name: meta.name.clone().unwrap_or_default(),
        phase,
        ready,
        restart_count,
        node: pod
            .spec
            .as_ref()
            .and_then(|s| s.node_name.clone()),
        started_at,
        conditions,
        container_statuses,
        oom_killed,
    }
}

// --- Ingress types ---

#[derive(Serialize, Debug)]
pub struct IngressSummary {
    pub name: String,
    pub namespace: String,
    pub hosts: Vec<String>,
    pub ingress_class: Option<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub addresses: Vec<String>,
}

#[derive(Serialize, Debug)]
pub struct IngressDetail {
    pub name: String,
    pub namespace: String,
    pub hosts: Vec<String>,
    pub ingress_class: Option<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub addresses: Vec<String>,
    pub rules: Vec<IngressRuleSummary>,
    pub tls: Vec<IngressTlsSummary>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Serialize, Debug)]
pub struct IngressRuleSummary {
    pub host: Option<String>,
    pub paths: Vec<IngressPathSummary>,
}

#[derive(Serialize, Debug)]
pub struct IngressPathSummary {
    pub path: String,
    pub path_type: String,
    pub service_name: String,
    pub service_port: i32,
}

#[derive(Serialize, Debug)]
pub struct IngressTlsSummary {
    pub hosts: Vec<String>,
    pub secret_name: Option<String>,
}

fn ingress_hosts(ing: &Ingress) -> Vec<String> {
    ing.spec
        .as_ref()
        .and_then(|s| s.rules.as_ref())
        .map(|rules| rules.iter().filter_map(|r| r.host.clone()).collect())
        .unwrap_or_default()
}

fn ingress_addresses(ing: &Ingress) -> Vec<String> {
    ing.status
        .as_ref()
        .and_then(|s| s.load_balancer.as_ref())
        .and_then(|lb| lb.ingress.as_ref())
        .map(|addrs| {
            addrs
                .iter()
                .filter_map(|a| a.hostname.clone().or_else(|| a.ip.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn ingress_rules(ing: &Ingress) -> Vec<IngressRuleSummary> {
    ing.spec
        .as_ref()
        .and_then(|s| s.rules.as_ref())
        .map(|rules| {
            rules
                .iter()
                .map(|r| IngressRuleSummary {
                    host: r.host.clone(),
                    paths: r
                        .http
                        .as_ref()
                        .map(|http| {
                            http.paths
                                .iter()
                                .map(|p| {
                                    let (svc_name, svc_port) = p
                                        .backend
                                        .service
                                        .as_ref()
                                        .map(|s| {
                                            let port = s
                                                .port
                                                .as_ref()
                                                .and_then(|p| p.number)
                                                .unwrap_or(80);
                                            (s.name.clone(), port)
                                        })
                                        .unwrap_or_default();
                                    IngressPathSummary {
                                        path: p.path.clone().unwrap_or_else(|| "/".to_string()),
                                        path_type: p.path_type.clone(),
                                        service_name: svc_name,
                                        service_port: svc_port,
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ingress_tls(ing: &Ingress) -> Vec<IngressTlsSummary> {
    ing.spec
        .as_ref()
        .and_then(|s| s.tls.as_ref())
        .map(|tls_list| {
            tls_list
                .iter()
                .map(|t| IngressTlsSummary {
                    hosts: t.hosts.clone().unwrap_or_default(),
                    secret_name: t.secret_name.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn ingress_summary(ing: &Ingress) -> IngressSummary {
    let meta = &ing.metadata;
    IngressSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        hosts: ingress_hosts(ing),
        ingress_class: ing.spec.as_ref().and_then(|s| s.ingress_class_name.clone()),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        addresses: ingress_addresses(ing),
    }
}

pub fn ingress_detail(ing: &Ingress) -> IngressDetail {
    let meta = &ing.metadata;
    IngressDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        hosts: ingress_hosts(ing),
        ingress_class: ing.spec.as_ref().and_then(|s| s.ingress_class_name.clone()),
        created_at: meta.creation_timestamp.as_ref().map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
        addresses: ingress_addresses(ing),
        rules: ingress_rules(ing),
        tls: ingress_tls(ing),
        annotations: meta.annotations.clone().unwrap_or_default(),
    }
}

// --- CronJob types ---

#[derive(Serialize, Debug)]
pub struct CronJobSummary {
    pub name: String,
    pub namespace: String,
    pub schedule: String,
    pub suspend: bool,
    pub active_count: i32,
    pub last_schedule_time: Option<String>,
    pub created_at: Option<String>,
    pub labels: BTreeMap<String, String>,
}

pub fn cronjob_summary(cj: &CronJob) -> CronJobSummary {
    let meta = &cj.metadata;
    let spec = cj.spec.as_ref();
    let status = cj.status.as_ref();

    CronJobSummary {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        schedule: spec.map(|s| s.schedule.clone()).unwrap_or_default(),
        suspend: spec.and_then(|s| s.suspend).unwrap_or(false),
        active_count: status
            .and_then(|s| s.active.as_ref())
            .map(|a| a.len() as i32)
            .unwrap_or(0),
        last_schedule_time: status
            .and_then(|s| s.last_schedule_time.as_ref())
            .map(|t| t.0.to_string()),
        created_at: meta
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
        labels: meta.labels.clone().unwrap_or_default(),
    }
}

// --- Node types ---

#[derive(Serialize, Debug)]
pub struct NodeConditionSummary {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct NodeSummary {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: Option<String>,
    pub memory_capacity: Option<String>,
    pub cpu_allocatable: Option<String>,
    pub memory_allocatable: Option<String>,
    pub os_image: Option<String>,
    pub kernel_version: Option<String>,
    pub kubelet_version: Option<String>,
    pub conditions: Vec<NodeConditionSummary>,
    pub created_at: Option<String>,
}

fn node_roles(node: &Node) -> Vec<String> {
    let labels = match node.metadata.labels.as_ref() {
        Some(l) => l,
        None => return Vec::new(),
    };
    let mut roles: Vec<String> = labels
        .keys()
        .filter_map(|k| {
            k.strip_prefix("node-role.kubernetes.io/")
                .map(|r| r.to_string())
        })
        .filter(|r| !r.is_empty())
        .collect();
    if roles.is_empty() {
        if let Some(role) = labels.get("kubernetes.io/role") {
            roles.push(role.clone());
        }
    }
    if roles.is_empty() {
        roles.push("<none>".to_string());
    }
    roles.sort();
    roles
}

fn node_ready_status(node: &Node) -> String {
    let conditions = match node.status.as_ref().and_then(|s| s.conditions.as_ref()) {
        Some(c) => c,
        None => return "Unknown".to_string(),
    };
    for c in conditions {
        if c.type_ == "Ready" {
            return match c.status.as_str() {
                "True" => "Ready".to_string(),
                "False" => "NotReady".to_string(),
                _ => "Unknown".to_string(),
            };
        }
    }
    "Unknown".to_string()
}

pub fn node_summary(node: &Node) -> NodeSummary {
    let meta = &node.metadata;
    let status = node.status.as_ref();

    let capacity = status.and_then(|s| s.capacity.as_ref());
    let allocatable = status.and_then(|s| s.allocatable.as_ref());
    let node_info = status.and_then(|s| s.node_info.as_ref());

    let conditions: Vec<NodeConditionSummary> = status
        .and_then(|s| s.conditions.as_ref())
        .map(|conds| {
            conds
                .iter()
                .map(|c| NodeConditionSummary {
                    condition_type: c.type_.clone(),
                    status: c.status.clone(),
                    reason: c.reason.clone(),
                    message: c.message.clone(),
                    last_transition: c.last_transition_time.as_ref().map(|t| t.0.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    NodeSummary {
        name: meta.name.clone().unwrap_or_default(),
        status: node_ready_status(node),
        roles: node_roles(node),
        cpu_capacity: capacity.and_then(|c| c.get("cpu")).map(|q| q.0.clone()),
        memory_capacity: capacity.and_then(|c| c.get("memory")).map(|q| q.0.clone()),
        cpu_allocatable: allocatable.and_then(|a| a.get("cpu")).map(|q| q.0.clone()),
        memory_allocatable: allocatable.and_then(|a| a.get("memory")).map(|q| q.0.clone()),
        os_image: node_info.map(|n| n.os_image.clone()),
        kernel_version: node_info.map(|n| n.kernel_version.clone()),
        kubelet_version: node_info.map(|n| n.kubelet_version.clone()),
        conditions,
        created_at: meta
            .creation_timestamp
            .as_ref()
            .map(|t| t.0.to_string()),
    }
}

// --- Application types ---

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationHealth {
    Healthy,
    Degraded,
    Unhealthy,
    Empty,
}

#[derive(Serialize, Debug)]
pub struct ApplicationSummary {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub created_at: Option<String>,
    pub deployment_count: usize,
    pub cronjob_count: usize,
    pub health: ApplicationHealth,
    pub gitops_enabled: bool,
}

#[derive(Serialize, Debug)]
pub struct ApplicationDetail {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub git: Option<ApplicationGitConfig>,
    pub deployments: Vec<DeploymentSummary>,
    pub cronjobs: Vec<CronJobSummary>,
    pub health: ApplicationHealth,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApplicationGitConfig {
    pub repo_url: String,
    pub branch: Option<String>,
    pub token_secret: Option<String>,
}

pub fn compute_application_health(deployments: &[DeploymentSummary]) -> ApplicationHealth {
    if deployments.is_empty() {
        return ApplicationHealth::Empty;
    }
    let all_available = deployments
        .iter()
        .all(|d| matches!(d.status, DeploymentPhase::Available));
    let any_available = deployments
        .iter()
        .any(|d| matches!(d.status, DeploymentPhase::Available));
    if all_available {
        ApplicationHealth::Healthy
    } else if any_available {
        ApplicationHealth::Degraded
    } else {
        ApplicationHealth::Unhealthy
    }
}

// --- Event types ---

#[derive(Serialize, Debug)]
pub struct EventSummary {
    pub namespace: String,
    pub name: String,
    pub event_type: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub involved_object_kind: String,
    pub involved_object_name: String,
    pub involved_object_namespace: Option<String>,
    pub count: Option<i32>,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub source_component: Option<String>,
    pub source_host: Option<String>,
}

pub fn event_summary(event: &k8s_openapi::api::core::v1::Event) -> EventSummary {
    let meta = &event.metadata;
    EventSummary {
        namespace: meta.namespace.clone().unwrap_or_default(),
        name: meta.name.clone().unwrap_or_default(),
        event_type: event.type_.clone().unwrap_or_else(|| "Normal".to_string()),
        reason: event.reason.clone(),
        message: event.message.clone(),
        involved_object_kind: event.involved_object.kind.clone().unwrap_or_default(),
        involved_object_name: event.involved_object.name.clone().unwrap_or_default(),
        involved_object_namespace: event.involved_object.namespace.clone(),
        count: event.count,
        first_timestamp: event.first_timestamp.as_ref().map(|t| t.0.to_string()),
        last_timestamp: event.last_timestamp.as_ref().map(|t| t.0.to_string()),
        source_component: event.source.as_ref().and_then(|s| s.component.clone()),
        source_host: event.source.as_ref().and_then(|s| s.host.clone()),
    }
}
