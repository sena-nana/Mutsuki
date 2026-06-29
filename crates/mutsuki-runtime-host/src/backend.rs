use mutsuki_runtime_contracts::{
    HostBackendDescriptor, PluginBackendDescriptor, PluginDeploymentKind,
};

use crate::clients::{ResourcePlanClient, TaskClient};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBackend {
    descriptor: HostBackendDescriptor,
}

impl HostBackend {
    pub fn new(descriptor: HostBackendDescriptor) -> Self {
        Self { descriptor }
    }

    pub fn descriptor(&self) -> &HostBackendDescriptor {
        &self.descriptor
    }

    pub fn supports_deployment(&self, deployment: &PluginDeploymentKind) -> bool {
        self.descriptor.supported_deployments.contains(deployment)
    }
}

#[derive(Debug)]
pub struct PluginBackend<T, R> {
    descriptor: PluginBackendDescriptor,
    task_client: T,
    resource_client: R,
}

impl<T, R> PluginBackend<T, R>
where
    T: TaskClient,
    R: ResourcePlanClient,
{
    pub fn new(descriptor: PluginBackendDescriptor, task_client: T, resource_client: R) -> Self {
        Self {
            descriptor,
            task_client,
            resource_client,
        }
    }

    pub fn descriptor(&self) -> &PluginBackendDescriptor {
        &self.descriptor
    }

    pub fn deployment_kind(&self) -> &PluginDeploymentKind {
        &self.descriptor.deployment_kind
    }

    pub fn task_client(&self) -> &T {
        &self.task_client
    }

    pub fn resource_client(&self) -> &R {
        &self.resource_client
    }
}
