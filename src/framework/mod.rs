pub mod laravel;

use std::path::Path;

/// Supported frameworks
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameworkKind {
    Laravel,
}

impl FrameworkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Laravel => "laravel",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "laravel" => Self::Laravel,
            _ => Self::Laravel, // default for now
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Laravel => "Laravel",
        }
    }
}

/// Framework-specific behavior
pub trait Framework {
    /// Detect if the given directory is this framework
    fn detect(project_path: &Path) -> bool;

    /// Services this framework provides beyond server + tunnel
    fn extra_services(&self) -> Vec<ServiceDef>;

    /// Framework-specific Caddyfile directives (inside the site block)
    fn caddyfile_directives(&self) -> String;
}

/// A background service the framework needs
#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub name: String,
    pub label_suffix: String,
    pub command: Vec<String>,
}

/// Detect which framework a project directory uses
pub fn detect(project_path: &Path) -> Option<FrameworkKind> {
    if laravel::Laravel::detect(project_path) {
        return Some(FrameworkKind::Laravel);
    }
    None
}
