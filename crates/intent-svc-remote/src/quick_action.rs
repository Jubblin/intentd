//! An orchestrator's quick actions on a machine (monorepo
//! `docs/protocol/methods/agent-aux.md`, "Quick actions on an
//! orchestrator", and `node-link.md`, "Quick actions on the machine"): the
//! first machine of the ordered list that can run the provider gets one
//! `oneShot` frame; nothing is retried on another machine.

use std::time::Duration;

use intent_core::machine_order::MachineOrderKey;
use intent_core::machines::{ExposedMachine, RemoteMachineState};
use intent_core::node_link::{ProviderStatus, ONE_SHOT_PROTOCOL};
use intent_services::feature_api::feature_ports::{
    MachineOneShotAnswer, MachineOneShotRequest, NO_QUICK_ACTION_MACHINE,
};
use intent_services::feature_api::Services;

use crate::RemoteServicesExt as _;

/// How long past the frame's own timeout the backend waits for the reply.
const REPLY_GRACE: Duration = Duration::from_secs(10);

/// Whether `row`'s last report offers `provider` for a quick action.
fn offers(row: &ExposedMachine, provider: &str, with_profile: bool) -> bool {
    row.capabilities.as_ref().is_some_and(|caps| {
        caps.providers.iter().any(|offered| {
            offered.id == provider
                && match offered.status {
                    ProviderStatus::SignedIn | ProviderStatus::Unknown => true,
                    ProviderStatus::SignedOut | ProviderStatus::AuthFailed => with_profile,
                    ProviderStatus::CliMissing
                    | ProviderStatus::PrerequisiteMissing
                    | ProviderStatus::Disabled => false,
                }
        })
    })
}

/// The candidates in list order: exposed machines, and the call's
/// workspace's running sandboxes at their entry's rank.
async fn candidates(svc: &Services, request: &MachineOneShotRequest) -> Vec<ExposedMachine> {
    let order = crate::order::current(svc).await.unwrap_or_default();
    let machines = svc.store().list_exposed_machines().await.unwrap_or_default();
    let sandboxes = match &request.workspace_id {
        Some(workspace) => svc
            .store()
            .list_cloud_machines()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|row| {
                row.cloud.as_ref().is_some_and(|cloud| {
                    cloud.workspace_id == workspace.as_str()
                        && cloud.state == intent_core::machines::CloudSandboxState::Running
                })
            })
            .collect(),
        None => Vec::new(),
    };
    let mut out = Vec::new();
    for row in order {
        match row.key {
            MachineOrderKey::Local => {}
            MachineOrderKey::Machine { node_id } => {
                out.extend(machines.iter().filter(|m| m.id == node_id).cloned());
            }
            MachineOrderKey::Cloud { cloud_machine_id } => {
                out.extend(
                    sandboxes
                        .iter()
                        .filter(|m| {
                            m.cloud
                                .as_ref()
                                .is_some_and(|cloud| cloud.cloud_machine_id == cloud_machine_id)
                        })
                        .cloned(),
                );
            }
        }
    }
    out.retain(|row| {
        row.state != RemoteMachineState::Revoked
            && request.node_id.as_ref().is_none_or(|pinned| &row.id == pinned)
            && offers(row, &request.provider, request.with_profile)
    });
    out
}

/// Run `request` on the first machine that takes it.
pub(crate) async fn machine_one_shot(
    svc: &Services,
    request: MachineOneShotRequest,
) -> MachineOneShotAnswer {
    let Some(feature) = svc.remote_feature() else {
        return MachineOneShotAnswer::NoMachine(NO_QUICK_ACTION_MACHINE.to_owned());
    };
    let listed = crate::order::current(svc)
        .await
        .is_ok_and(|order| order.iter().any(|row| row.key != MachineOrderKey::Local));
    if !listed {
        return MachineOneShotAnswer::NoMachine(NO_QUICK_ACTION_MACHINE.to_owned());
    }
    let mut outdated = None;
    for row in candidates(svc, &request).await {
        let Ok(node) = feature.node(svc, &row.id).await else {
            continue;
        };
        if node.shared_ref().current_link().is_none() && node.connect().await.is_err() {
            continue;
        }
        if node.shared_ref().protocol() < ONE_SHOT_PROTOCOL {
            outdated.get_or_insert_with(|| row.name.clone());
            continue;
        }
        let wait = Duration::from_millis(request.frame.timeout_ms) + REPLY_GRACE;
        let machine = node.status().node_name;
        return match node.one_shot(request.frame, wait).await {
            Ok(reply) => MachineOneShotAnswer::Replied {
                node_id: row.id,
                machine,
                reply,
            },
            Err(error) => {
                tracing::info!(machine = %row.id, %error, "quick action: the machine's link dropped before its reply");
                MachineOneShotAnswer::Lost { machine }
            }
        };
    }
    MachineOneShotAnswer::NoMachine(match outdated {
        Some(name) => format!("update Intent on {name} to run quick actions there"),
        None => "no machine for quick actions is online".to_owned(),
    })
}
