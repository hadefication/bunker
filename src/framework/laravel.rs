use std::path::Path;

use super::{Framework, ServiceDef};

pub struct Laravel {
    pub php_path: String,
    pub project_path: String,
    pub scheduler_enabled: bool,
}

impl Framework for Laravel {
    fn detect(project_path: &Path) -> bool {
        project_path.join("artisan").exists()
    }

    fn extra_services(&self) -> Vec<ServiceDef> {
        let mut services = vec![ServiceDef {
            name: "queue".to_string(),
            label_suffix: "queue".to_string(),
            command: vec![
                self.php_path.clone(),
                format!("{}/artisan", self.project_path),
                "queue:work".to_string(),
                "--tries=3".to_string(),
                "--timeout=30".to_string(),
            ],
        }];

        if self.scheduler_enabled {
            services.push(ServiceDef {
                name: "scheduler".to_string(),
                label_suffix: "scheduler".to_string(),
                command: vec![
                    self.php_path.clone(),
                    format!("{}/artisan", self.project_path),
                    "schedule:work".to_string(),
                ],
            });
        }

        services
    }

    fn caddyfile_directives(&self) -> String {
        r#"    # Block sensitive paths
    @blocked {
        path /vendor/* /storage/* /artisan
        path *.php~* *.swp *.bak *.orig
    }
    respond @blocked 404

    # Block dotfiles but allow /.well-known/* (RFC 8615)
    @dotfiles {
        path /.*
        not path /.well-known/*
    }
    respond @dotfiles 404

    # Block direct PHP file access (only index.php via php_server rewrite)
    @directPhp {
        path *.php
        not path /index.php
    }
    respond @directPhp 404

    php_server"#
            .to_string()
    }
}
