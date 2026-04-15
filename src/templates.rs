use crate::config::ProjectConfig;
use crate::framework::laravel::Laravel;
use crate::framework::Framework;

pub fn cloudflared_config(config: &ProjectConfig) -> String {
    format!(
        r#"tunnel: {tunnel_uuid}
credentials-file: {cred_file}

ingress:
  - hostname: {domain}
    service: http://localhost:{port}
  - service: http_status:404
"#,
        tunnel_uuid = config.tunnel_uuid,
        cred_file = format!(
            "{}/.cloudflared/{}.json",
            std::env::var("HOME").unwrap_or_default(),
            config.tunnel_uuid
        ),
        domain = config.domain,
        port = config.port,
    )
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| {
            // Strip XML 1.0 forbidden control characters (U+0000-0008, U+000B, U+000C, U+000E-001F)
            !matches!(*c as u32, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F)
        })
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            _ => c.to_string(),
        })
        .collect()
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
    root * "{project_path}/public"

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
        output file "{logs_dir}/caddy-access.log" {{
            roll_size 10MiB
            roll_keep 5
        }}
        format json {{
            delete request>headers>Authorization
            delete request>headers>Cookie
            delete request>headers>Set-Cookie
            delete request>headers>X-Api-Key
        }}
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
    let server_label = format!("com.bunker.{}.server", config.project_name);
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
    let tunnel_label = format!("com.bunker.{}.tunnel", config.project_name);
    let tunnel_plist = plist(
        &tunnel_label,
        &[
            config.cloudflared_path.clone(),
            "tunnel".to_string(),
            "--no-autoupdate".to_string(),
            "--config".to_string(),
            config.project_dir().join("cloudflared.yml").display().to_string(),
            "run".to_string(),
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
        let label = format!("com.bunker.{}.{}", config.project_name, svc.label_suffix);
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

    // Log rotation plist — runs daily, keeps 5 rotated copies, 10MB max per file
    let logrotate_label = format!("com.bunker.{}.logrotate", config.project_name);
    let logrotate_plist = log_rotation_plist(&logrotate_label, &logs_str);
    plists.push((format!("{}.plist", logrotate_label), logrotate_plist));

    plists
}

fn log_rotation_plist(label: &str, logs_dir: &str) -> String {
    // Rotate .log files over 10MB, keep 5 old copies, remove oldest
    let script = format!(
        r#"for f in "{logs_dir}"/*.log; do \
  [ -f "$f" ] || continue; \
  size=$(stat -f%z "$f" 2>/dev/null || echo 0); \
  if [ "$size" -gt 10485760 ]; then \
    for i in 4 3 2 1; do \
      j=$((i+1)); \
      [ -f "$f.$i" ] && mv "$f.$i" "$f.$j"; \
    done; \
    cp "$f" "$f.1" && : > "$f"; \
    [ -f "$f.5" ] && rm "$f.5"; \
  fi; \
done"#,
        logs_dir = logs_dir,
    );

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/sh</string>
        <string>-c</string>
        <string>{script}</string>
    </array>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key>
        <integer>3</integer>
        <key>Minute</key>
        <integer>0</integer>
    </dict>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
</dict>
</plist>
"#,
        label = xml_escape(label),
        script = xml_escape(&script),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::FrameworkKind;

    fn test_config() -> ProjectConfig {
        ProjectConfig {
            project_name: "test-app".to_string(),
            project_path: "/tmp/test-app".to_string(),
            port: 8700,
            domain: "test.example.com".to_string(),
            tunnel_name: "test-app".to_string(),
            tunnel_uuid: "00000000-0000-0000-0000-000000000000".to_string(),
            php_path: "/usr/bin/php".to_string(),
            frankenphp_path: "/usr/local/bin/frankenphp".to_string(),
            cloudflared_path: "/usr/local/bin/cloudflared".to_string(),
            scheduler_enabled: false,
            framework: FrameworkKind::Laravel,
        }
    }

    #[test]
    fn xml_escape_special_chars() {
        assert_eq!(xml_escape("<test>&\"'"), "&lt;test&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn xml_escape_strips_control_chars() {
        assert_eq!(xml_escape("hello\x08world"), "helloworld");
        assert_eq!(xml_escape("a\x0Bb\x0Cc"), "abc");
        // Tab, newline, carriage return are allowed in XML 1.0
        assert_eq!(xml_escape("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn xml_escape_clean_string() {
        assert_eq!(xml_escape("hello"), "hello");
    }

    #[test]
    fn caddyfile_contains_port() {
        let config = test_config();
        let cf = caddyfile(&config);
        assert!(cf.contains(":8700 {"));
    }

    #[test]
    fn caddyfile_quoted_paths() {
        let config = test_config();
        let cf = caddyfile(&config);
        assert!(cf.contains(r#"root * "/tmp/test-app/public""#));
    }

    #[test]
    fn caddyfile_has_hsts() {
        let config = test_config();
        let cf = caddyfile(&config);
        assert!(cf.contains("Strict-Transport-Security"));
    }

    #[test]
    fn caddyfile_blocks_direct_php() {
        let config = test_config();
        let cf = caddyfile(&config);
        assert!(cf.contains("@directPhp"));
    }

    #[test]
    fn caddyfile_strips_sensitive_headers() {
        let config = test_config();
        let cf = caddyfile(&config);
        assert!(cf.contains("delete request>headers>Authorization"));
        assert!(cf.contains("delete request>headers>Cookie"));
    }

    #[test]
    fn cloudflared_config_has_ingress() {
        let config = test_config();
        let cf = cloudflared_config(&config);
        assert!(cf.contains("ingress:"));
        assert!(cf.contains("hostname: test.example.com"));
        assert!(cf.contains("http://localhost:8700"));
        assert!(cf.contains("http_status:404"));
    }

    #[test]
    fn cloudflared_config_has_credentials() {
        let config = test_config();
        let cf = cloudflared_config(&config);
        assert!(cf.contains("tunnel: 00000000-0000-0000-0000-000000000000"));
        assert!(cf.contains("credentials-file:"));
    }

    #[test]
    fn plist_xml_escapes_values() {
        let p = plist(
            "com.test.server",
            &["/path/with <special>&chars".to_string()],
            "/tmp/test",
            "/tmp/stdout.log",
            "/tmp/stderr.log",
            false,
        );
        assert!(p.contains("&lt;special&gt;&amp;chars"));
        assert!(!p.contains("<special>"));
    }

    #[test]
    fn generate_plists_count_without_scheduler() {
        let config = test_config();
        let plists = generate_plists(&config);
        assert_eq!(plists.len(), 4); // server, tunnel, queue, logrotate
    }

    #[test]
    fn generate_plists_count_with_scheduler() {
        let mut config = test_config();
        config.scheduler_enabled = true;
        let plists = generate_plists(&config);
        assert_eq!(plists.len(), 5); // server, tunnel, queue, scheduler, logrotate
    }

    #[test]
    fn logrotate_plist_has_calendar_interval() {
        let config = test_config();
        let plists = generate_plists(&config);
        let (name, content) = plists.last().unwrap();
        assert!(name.contains("logrotate"));
        assert!(content.contains("StartCalendarInterval"));
        assert!(content.contains("<integer>3</integer>")); // 3 AM
    }
}
