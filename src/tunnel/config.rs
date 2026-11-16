//! `~/.bunker/<project>/cloudflared.yml`.
use std::path::Path;

/// The tunnel serves only the project's domain, to the web forward; anything else is a 404.
pub fn render_cloudflared_yml(id: &str, creds: &Path, domain: &str, web_port: u16) -> String {
    format!(
        "tunnel: {id}\ncredentials-file: {}\ningress:\n  - hostname: {domain}\n    service: http://127.0.0.1:{web_port}\n  - service: http_status:404\n",
        creds.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yml_routes_domain_to_web_port_with_404_fallback() {
        let y = render_cloudflared_yml(
            "6ff42ae2-765d-4adf-8112-31c55c1551ef",
            Path::new("/Users/x/.bunker/life-os/tunnel-credentials.json"),
            "life-os.bangkila.com",
            18900,
        );
        assert_eq!(
            y,
            concat!(
                "tunnel: 6ff42ae2-765d-4adf-8112-31c55c1551ef\n",
                "credentials-file: /Users/x/.bunker/life-os/tunnel-credentials.json\n",
                "ingress:\n",
                "  - hostname: life-os.bangkila.com\n",
                "    service: http://127.0.0.1:18900\n",
                "  - service: http_status:404\n",
            )
        );
    }
}
