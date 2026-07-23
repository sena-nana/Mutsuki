use mutsuki_web_extension::{WebExtensionContext, WebServiceContext};
use mutsuki_web_protocol::{WebApplicationDescriptor, WebShellAssets};

/// Product-level web application assembled by WebHost.
pub trait WebApplication: Send + Sync {
    fn descriptor(&self) -> WebApplicationDescriptor;
    fn shell(&self) -> WebShellAssets;
    fn register_services(&self, ctx: &mut WebServiceContext);
    fn register_extensions(&self, ctx: &mut WebExtensionContext);
}

/// Minimal application used by tests and recovery-first boots.
#[derive(Debug, Clone)]
pub struct MinimalWebApplication {
    descriptor: WebApplicationDescriptor,
    shell: WebShellAssets,
}

impl MinimalWebApplication {
    pub fn new(descriptor: WebApplicationDescriptor, shell: WebShellAssets) -> Self {
        Self { descriptor, shell }
    }

    pub fn empty(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            descriptor: WebApplicationDescriptor {
                id: id.clone(),
                name: id,
                version: "0.1.0".into(),
                brand: Some("Mutsuki".into()),
                theme: Some("default".into()),
            },
            shell: WebShellAssets {
                root_dir: std::path::PathBuf::from("/nonexistent-shell"),
                index_file: "index.html".into(),
                import_map: Default::default(),
            },
        }
    }
}

impl WebApplication for MinimalWebApplication {
    fn descriptor(&self) -> WebApplicationDescriptor {
        self.descriptor.clone()
    }

    fn shell(&self) -> WebShellAssets {
        self.shell.clone()
    }

    fn register_services(&self, _ctx: &mut WebServiceContext) {}

    fn register_extensions(&self, _ctx: &mut WebExtensionContext) {}
}
