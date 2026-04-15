use crate::config::ProjectConfig;
use crate::framework::laravel::Laravel;
use crate::framework::Framework;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn caddyfile(config: &ProjectConfig) -> String {
    let framework_directives = match config.framework {
        crate::framework::FrameworkKind::Laravel => {
            let laravel = Laravel {
                php_path: config.php_path.clone(),
                project_path: config.project_path.clone(),
                scheduler_enabled: config.scheduler_enabled,
            };
            laravel.caddyfile_directives()
        }
    };

    format!(
        r#":{port} {{
    root * {project_path}/public

    # Security headers
    header {{
        X-Content-Type-Options "nosniff"
        X-Frame-Options "DENY"
        Referrer-Policy "strict-origin-when-cross-origin"
        Permissions-Policy "camera=(), microphone=(), geolocation=()"
        Strict-Transport-Security "max-age=31536000; includeSubDomains; preload"
        -Server
    }}

{framework_directives}

    # Compression
    encode zstd gzip

    # Request limits
    request_body {{
        max_size 10MB
    }}

    # Access logging
    log {{
        output file {logs_dir}/caddy-access.log {{
            roll_size 10MiB
            roll_keep 5
        }}
        format json
    }}
}}
"#,
        port = config.port,
        project_path = config.project_path,
        framework_directives = framework_directives,
        logs_dir = config.logs_dir().display(),
    )
}

pub fn plist(
    label: &str,
    program_args: &[String],
    working_dir: &str,
    stdout_log: &str,
    stderr_log: &str,
    with_env: bool,
) -> String {
    let args_xml: String = program_args
        .iter()
        .map(|a| format!("        <string>{}</string>", xml_escape(a)))
        .collect::<Vec<_>>()
        .join("\n");

    let env_block = if with_env {
        r#"
    <key>EnvironmentVariables</key>
    <dict>
        <key>APP_ENV</key>
        <string>production</string>
    </dict>"#
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
    <key>WorkingDirectory</key>
    <string>{working_dir}</string>{env_block}
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout_log}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>
</dict>
</plist>
"#,
        label = xml_escape(label),
        args_xml = args_xml,
        working_dir = xml_escape(working_dir),
        env_block = env_block,
        stdout_log = xml_escape(stdout_log),
        stderr_log = xml_escape(stderr_log),
    )
}

/// Generate all plists for a project config
pub fn generate_plists(config: &ProjectConfig) -> Vec<(String, String)> {
    let logs = config.logs_dir();
    let logs_str = logs.display().to_string();
    let mut plists = Vec::new();

    // Server plist
    let server_label = format!("com.{}.server", config.project_name);
    let server_plist = plist(
        &server_label,
        &[
            config.frankenphp_path.clone(),
            "run".to_string(),
            "--config".to_string(),
            config.project_dir().join("Caddyfile").display().to_string(),
        ],
        &config.project_path,
        &format!("{}/frankenphp-stdout.log", logs_str),
        &format!("{}/frankenphp-stderr.log", logs_str),
        true,
    );
    plists.push((format!("{}.plist", server_label), server_plist));

    // Tunnel plist
    let tunnel_label = format!("com.{}.tunnel", config.project_name);
    let tunnel_plist = plist(
        &tunnel_label,
        &[
            config.cloudflared_path.clone(),
            "tunnel".to_string(),
            "--no-autoupdate".to_string(),
            "run".to_string(),
            "--url".to_string(),
            format!("localhost:{}", config.port),
            config.tunnel_name.clone(),
        ],
        &config.project_path,
        &format!("{}/cloudflared-stdout.log", logs_str),
        &format!("{}/cloudflared-stderr.log", logs_str),
        false,
    );
    plists.push((format!("{}.plist", tunnel_label), tunnel_plist));

    // Framework-specific services
    let framework_services = match config.framework {
        crate::framework::FrameworkKind::Laravel => {
            let laravel = Laravel {
                php_path: config.php_path.clone(),
                project_path: config.project_path.clone(),
                scheduler_enabled: config.scheduler_enabled,
            };
            laravel.extra_services()
        }
    };

    for svc in framework_services {
        let label = format!("com.{}.{}", config.project_name, svc.label_suffix);
        let svc_plist = plist(
            &label,
            &svc.command,
            &config.project_path,
            &format!("{}/{}-stdout.log", logs_str, svc.name),
            &format!("{}/{}-stderr.log", logs_str, svc.name),
            true,
        );
        plists.push((format!("{}.plist", label), svc_plist));
    }

    plists
}
