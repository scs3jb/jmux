//! CLI relay server — enables running jmux commands from within remote SSH sessions.
//!
//! Architecture:
//! 1. Local relay server listens on TCP (ephemeral port) with HMAC-SHA256 auth
//! 2. SSH reverse tunnel forwards a remote port to the local relay
//! 3. Remote jmux wrapper dials the relay port to send commands
//! 4. Relay forwards commands to the local jmux Unix socket

use super::bootstrap::{remote_shell_path, ssh_command};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Maximum simultaneous relay connections.  Excess connections are rejected
/// immediately to prevent resource exhaustion from an untrusted remote host.
const MAX_RELAY_CONNECTIONS: usize = 8;
static RELAY_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

/// A relay server that accepts authenticated commands and forwards them
/// to the local jmux socket.
pub struct RelayServer {
    local_port: u16,
    relay_id: String,
    auth_token: String,
    alive: Arc<AtomicBool>,
    reverse_tunnel: Option<Child>,
}

impl RelayServer {
    /// Start a relay server on an ephemeral localhost port.
    pub fn start(local_socket_path: &str) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to bind relay listener: {}", e))?;
        let local_port = listener
            .local_addr()
            .map_err(|e| format!("Failed to get local addr: {}", e))?
            .port();

        let relay_id = uuid::Uuid::new_v4().to_string();
        // Token: 16 random bytes hex-encoded (32 hex chars, no hyphens).
        // Go side calls hex.DecodeString() which fails on UUID hyphens.
        let auth_token = crate::socket::auth::hex_encode(&uuid::Uuid::new_v4().into_bytes());
        let alive = Arc::new(AtomicBool::new(true));

        let alive_clone = Arc::clone(&alive);
        let socket_path = local_socket_path.to_string();
        let relay_id_clone = relay_id.clone();
        let auth_token_clone = auth_token.clone();

        std::thread::spawn(move || {
            tracing::info!(port = local_port, "Relay server listening");

            loop {
                if !alive_clone.load(Ordering::Acquire) {
                    break;
                }

                match listener.accept() {
                    Ok((stream, addr)) => {
                        // Reject connections beyond the limit before spawning a thread.
                        let count = RELAY_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                        if count >= MAX_RELAY_CONNECTIONS {
                            RELAY_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                            tracing::warn!(?addr, "Relay: connection limit reached, rejecting");
                            continue;
                        }
                        tracing::debug!(?addr, "Relay: new connection");
                        let socket = socket_path.clone();
                        let rid = relay_id_clone.clone();
                        let token = auth_token_clone.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = handle_relay_connection(stream, &socket, &rid, &token) {
                                tracing::debug!("Relay connection error: {}", e);
                            }
                            RELAY_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        tracing::warn!("Relay accept error: {}", e);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            tracing::info!("Relay server stopped");
        });

        Ok(Self {
            local_port,
            relay_id,
            auth_token,
            alive,
            reverse_tunnel: None,
        })
    }

    /// The relay ID for authentication.
    pub fn relay_id(&self) -> &str {
        &self.relay_id
    }

    /// Start the SSH reverse tunnel and install metadata on the remote host.
    ///
    /// The remote port is allocated by SSH (`0` means ephemeral).
    /// Returns the remote port that was allocated.
    pub fn start_reverse_tunnel(
        &mut self,
        ssh_args: &[String],
        remote_daemon_path: &str,
    ) -> Result<u16, String> {
        // Pick the first free port in the configured range on the remote.
        let remote_port = allocate_remote_port(ssh_args)?;

        let forward_spec = format!("127.0.0.1:{}:127.0.0.1:{}", remote_port, self.local_port);

        tracing::info!(
            forward = %forward_spec,
            "Starting SSH reverse tunnel"
        );

        // Deliberately NO ExitOnForwardFailure: this connection inherits the
        // user's ~/.ssh/config, which may carry an unrelated LocalForward (e.g.
        // `LocalForward 8443`) already bound by the interactive session. With
        // ExitOnForwardFailure=yes that collision would tear down the whole
        // tunnel — and the CLI relay with it. Without it, a failed LocalForward
        // is just a harmless warning and our `-R` is what matters. We verify the
        // reverse forward came up below instead of relying on the exit code.
        let child = Command::new("ssh")
            .args(["-N", "-T", "-S", "none"])
            .args(["-o", "ConnectTimeout=6"])
            .args(["-R", &forward_spec])
            .args(ssh_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start reverse tunnel: {}", e))?;

        self.reverse_tunnel = Some(child);

        // Wait briefly for the tunnel to establish
        std::thread::sleep(Duration::from_millis(500));

        // Install metadata on remote
        install_remote_metadata(
            ssh_args,
            remote_port,
            &self.relay_id,
            &self.auth_token,
            remote_daemon_path,
        )?;

        // Verify the reverse forward is actually listening — without
        // ExitOnForwardFailure a failed `-R` would otherwise be silent. The
        // probe clears forwardings so it can't contend the session's ports.
        let listening = ssh_command(
            ssh_args,
            &format!("ss -tlnH 2>/dev/null | grep -q ':{remote_port} '"),
            4,
        )
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
        if !listening {
            tracing::warn!(
                remote_port,
                "reverse tunnel port not listening on remote; CLI relay may be unavailable"
            );
        }

        Ok(remote_port)
    }

    /// Stop the relay server and reverse tunnel.
    pub fn stop(&mut self) {
        self.alive.store(false, Ordering::Release);
        // Unblock accept
        let _ = TcpStream::connect(format!("127.0.0.1:{}", self.local_port));
        if let Some(mut child) = self.reverse_tunnel.take() {
            let _ = child.kill();
        }
    }
}

impl Drop for RelayServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Handle a single relay client connection with HMAC-SHA256 challenge-response auth.
fn handle_relay_connection(
    mut stream: TcpStream,
    socket_path: &str,
    relay_id: &str,
    auth_token: &str,
) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    // Step 1: Send challenge
    let nonce = uuid::Uuid::new_v4().to_string();
    let challenge = serde_json::json!({
        "protocol": "jmux-relay-auth",
        "version": 1,
        "relay_id": relay_id,
        "nonce": nonce,
    });
    let challenge_line = serde_json::to_string(&challenge).expect("challenge JSON");
    writeln!(stream, "{}", challenge_line).map_err(|e| format!("write challenge: {}", e))?;
    stream.flush().ok();

    // Step 2: Read auth response (bounded: 32 KB — large enough for any valid JSON auth frame)
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let response_line = read_limited_line(&mut reader, 32 * 1024)?;

    let response: serde_json::Value = serde_json::from_str(response_line.trim())
        .map_err(|e| format!("parse auth response: {}", e))?;

    let client_relay_id = response
        .get("relay_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let client_mac = response.get("mac").and_then(|v| v.as_str()).unwrap_or("");

    if client_relay_id != relay_id {
        return Err("Relay ID mismatch".to_string());
    }

    // Step 3: Verify HMAC-SHA256 with constant-time comparison.
    // Hex-decode the token so both Rust and Go use the same raw key bytes.
    let key = crate::socket::auth::hex_decode(auth_token)
        .ok_or_else(|| "auth_token is not valid hex".to_string())?;
    let message = format!("relay_id={}\nnonce={}\nversion=1", relay_id, nonce);
    if !crate::socket::auth::verify_hmac_raw(&key, message.as_bytes(), client_mac) {
        return Err("HMAC verification failed".to_string());
    }

    // Step 4: Send auth OK — Go's authenticateRelayConn expects {"ok":true} before
    // proceeding to send commands.
    writeln!(stream, r#"{{"ok":true}}"#).map_err(|e| format!("write auth ok: {}", e))?;
    stream.flush().ok();

    // Step 5: Read command (bounded: 1 MB — enough for any realistic socket v2 request)
    let command_line = read_limited_line(&mut reader, 1024 * 1024)?;

    if command_line.trim().is_empty() {
        return Err("Empty command".to_string());
    }

    // Forward to local jmux socket
    let response = forward_to_socket(socket_path, command_line.trim())?;

    // Send response back to client
    writeln!(stream, "{}", response).map_err(|e| format!("write response: {}", e))?;
    stream.flush().ok();

    Ok(())
}

/// Read one newline-terminated line from `reader`, capping at `limit` content bytes.
///
/// Returns the line including its trailing `\n`.  Returns an error if the line
/// exceeds `limit` bytes (no newline found within the window), preventing OOM
/// from an unbounded pre-auth read.
fn read_limited_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<String, String> {
    let mut buf = String::new();
    reader
        .by_ref()
        .take((limit + 1) as u64)
        .read_line(&mut buf)
        .map_err(|e| format!("read error: {e}"))?;
    // If the buffer filled the window without a newline, the line is too long.
    if !buf.ends_with('\n') && buf.len() >= limit {
        return Err(format!("line exceeds {limit} bytes"));
    }
    Ok(buf)
}

/// Forward a command to the local jmux Unix socket.
fn forward_to_socket(socket_path: &str, command: &str) -> Result<String, String> {
    use std::os::unix::net::UnixStream;

    let mut sock =
        UnixStream::connect(socket_path).map_err(|e| format!("Connect to socket: {}", e))?;
    sock.set_read_timeout(Some(Duration::from_secs(5))).ok();

    writeln!(sock, "{}", command).map_err(|e| format!("Write to socket: {}", e))?;
    sock.flush().ok();

    let mut response = String::new();
    let mut buf = [0u8; 8192];
    loop {
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(format!("Read from socket: {}", e)),
        }
    }

    Ok(response.trim().to_string())
}

/// Find an available port on the remote host for the reverse tunnel.
///
/// Returns the first free port in the configured `remote_relay_ports` range
/// (default 10000–10100), so concurrent remote sessions don't collide. The
/// whole range is checked in a single SSH round-trip: `ss` dumps the listening
/// ports once and an `awk` one-liner picks the first port not in use — far
/// cheaper than one SSH handshake per port.
fn allocate_remote_port(ssh_args: &[String]) -> Result<u16, String> {
    let range = crate::settings::load().remote_relay_ports;
    let (start, end) = range.bounds();
    if range.end < range.start {
        tracing::warn!(
            configured_start = range.start,
            configured_end = range.end,
            used_start = start,
            used_end = end,
            "remote_relay_ports has end < start; falling back to the default range"
        );
    }

    // Single remote command: collect listening ports from `ss`, then print the
    // first port in [start, end] that isn't among them. `$4` is local
    // address:port; the port is the field after the last colon (handles IPv4,
    // IPv6 and `*:port`). Awk is single-quoted so the remote shell leaves it be.
    let script = format!(
        "ss -tlnH 2>/dev/null | awk '{{n=split($4,a,\":\"); u[a[n]]=1}} \
         END{{for(p={start};p<={end};p++) if(!(p in u)){{print p; exit}}}}'"
    );
    if let Ok(out) = ssh_command(ssh_args, &script, 4).output() {
        if let Ok(port) = String::from_utf8_lossy(&out.stdout).trim().parse::<u16>() {
            if (start..=end).contains(&port) {
                return Ok(port);
            }
        }
    }

    // Fallback: no free port found (or the probe failed) — use the low end and
    // let the reverse tunnel's ExitOnForwardFailure surface a real collision.
    tracing::warn!(
        start,
        end,
        "no free remote port found in range; falling back to {start}"
    );
    Ok(start)
}

/// Install relay metadata files on the remote host.
///
/// Writes a JSON auth file (`~/.jmux/relay/<port>.auth`) with restrictive
/// permissions so other users cannot read the relay token.  The JSON format
/// matches Go's `relayAuthState` struct (`relay_id` / `relay_token`).
fn install_remote_metadata(
    ssh_args: &[String],
    remote_port: u16,
    relay_id: &str,
    auth_token: &str,
    daemon_path: &str,
) -> Result<(), String> {
    // Shell-escape the relay credentials even though they are safe ASCII;
    // this provides defence-in-depth if the generation ever changes.
    let esc_relay_id = shell_escape::escape(relay_id.into());
    let esc_auth_token = shell_escape::escape(auth_token.into());
    // Symlink target for the daemon. `remote_shell_path` expands a leading `~/`
    // to `"$HOME"/…` so the remote shell resolves it (plain shell_escape would
    // quote the tilde and create a literal `~` path).
    let daemon_target = remote_shell_path(daemon_path);

    // Write JSON: {"relay_id":"...","relay_token":"..."} (matches Go relayAuthState)
    // chmod 700 the relay dir and 600 the auth file so only the owner can read them.
    //
    // Then install the interactive bridge (Stage 2): symlink the daemon binary as
    // `jmux` so it runs in CLI mode, write a `jmux-env.sh` that puts ~/.jmux/bin on
    // PATH, and idempotently source it from the remote login shells. We deliberately
    // do NOT export JMUX_SOCKET_PATH: the CLI treats that variable as a literal
    // socket address (a `/`-prefixed value is dialed as a Unix socket), so pointing
    // it at the socket_addr *file* breaks every command. Leaving it unset makes the
    // CLI read the relay address from ~/.jmux/socket_addr and enables its
    // stale-address refresh when a later session rewrites that file with a new port.
    let script = format!(
        r#"mkdir -p ~/.jmux/relay ~/.jmux/bin && chmod 700 ~/.jmux/relay
echo '127.0.0.1:{remote_port}' > ~/.jmux/socket_addr
printf '{{"relay_id":"%s","relay_token":"%s"}}' {esc_relay_id} {esc_auth_token} > ~/.jmux/relay/{remote_port}.auth
chmod 600 ~/.jmux/relay/{remote_port}.auth
ln -sf {daemon_target} "$HOME/.jmux/bin/jmux"
cat > "$HOME/.jmux/bin/jmux-env.sh" <<'JMUX_ENV_EOF'
export PATH="$HOME/.jmux/bin:$PATH"
# Panel/workspace ids forwarded over SSH as LC_* (stock sshd AcceptEnv LANG LC_*).
[ -n "$LC_JMUX_PANEL_ID" ] && export JMUX_PANEL_ID="$LC_JMUX_PANEL_ID"
[ -n "$LC_JMUX_WORKSPACE_ID" ] && export JMUX_WORKSPACE_ID="$LC_JMUX_WORKSPACE_ID"
# Wrap `claude` so each launch pins a session id and reports it to jmux via the
# relay CLI, letting a restored remote tab resume that exact conversation. Runs
# in a subshell so no state leaks; skips injection when a session is already
# selected. Best-effort: with no panel id or `jmux` CLI, it just runs claude.
if command -v claude >/dev/null 2>&1; then
claude() (
  for _jmux_a in "$@"; do
    case "$_jmux_a" in
      -r|--resume|--resume=*|-c|--continue|--session-id|--session-id=*|--fork-session)
        command claude "$@"; exit ;;
    esac
  done
  _jmux_sid=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen 2>/dev/null | tr '[:upper:]' '[:lower:]')
  if [ -n "$_jmux_sid" ] && [ -n "$JMUX_PANEL_ID" ] && command -v jmux >/dev/null 2>&1; then
    jmux rpc workspace.report_agent_session "{{\"agent\":\"claude\",\"session_id\":\"$_jmux_sid\",\"surface\":\"$JMUX_PANEL_ID\"}}" >/dev/null 2>&1 &
    command claude --session-id "$_jmux_sid" "$@"
    exit
  fi
  command claude "$@"
)
fi
JMUX_ENV_EOF
chmod 600 "$HOME/.jmux/bin/jmux-env.sh"
for rc in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "$HOME/.profile"; do
  [ -e "$rc" ] || continue
  grep -q jmux-env.sh "$rc" 2>/dev/null && continue
  printf '\n# >>> jmux >>>\n[ -f "$HOME/.jmux/bin/jmux-env.sh" ] && . "$HOME/.jmux/bin/jmux-env.sh"\n# <<< jmux <<<\n' >> "$rc"
done"#,
        remote_port = remote_port,
        esc_relay_id = esc_relay_id,
        esc_auth_token = esc_auth_token,
        daemon_target = daemon_target,
    );

    let status = ssh_command(ssh_args, script.trim(), 6)
        .status()
        .map_err(|e| format!("Failed to install relay metadata: {}", e))?;

    if !status.success() {
        return Err("Failed to install relay metadata on remote".to_string());
    }

    tracing::info!(remote_port, "Relay metadata installed on remote");
    Ok(())
}
