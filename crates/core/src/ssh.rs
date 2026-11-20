//! SSH local port-forward via russh. Keeps the tunnel alive for the
//! lifetime of the returned [`SshTunnel`].

use std::net::SocketAddr;
use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::ChannelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::conn::SshConfig;
use crate::error::{MyDbError, Result};

/// Live local forward. Dropping this shuts the listener (and eventually the session).
pub struct SshTunnel {
    /// Bound `127.0.0.1:ephemeral` address clients should connect to.
    pub local_addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

struct AcceptAll;

impl client::Handler for AcceptAll {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _key: &russh::keys::PublicKey,
    ) -> core::result::Result<bool, Self::Error> {
        // Host-key verification lands with a known_hosts store in a later pass.
        // Production users should prefer a dedicated bastion identity.
        Ok(true)
    }
}

/// Open a local port-forward to `dest_host:dest_port` through `ssh`.
pub async fn open_tunnel(ssh: &SshConfig, dest_host: &str, dest_port: u16) -> Result<SshTunnel> {
    let port = if ssh.port == 0 { 22 } else { ssh.port };
    let config = Arc::new(client::Config::default());
    let mut session = client::connect(config, (ssh.host.as_str(), port), AcceptAll)
        .await
        .map_err(|e| MyDbError::Ssh(e.to_string()))?;

    authenticate(&mut session, ssh).await?;
    let session = Arc::new(session);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| MyDbError::Ssh(format!("cannot bind local forward: {e}")))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| MyDbError::Ssh(e.to_string()))?;

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let dest_host = dest_host.to_string();
    let handle = Arc::clone(&session);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break; };
                    let handle = Arc::clone(&handle);
                    let dest_host = dest_host.clone();
                    tokio::spawn(async move {
                        if let Err(err) = pump(handle, stream, &dest_host, dest_port).await {
                            tracing::debug!("ssh forward closed: {err}");
                        }
                    });
                }
            }
        }
        drop(session);
    });

    Ok(SshTunnel {
        local_addr,
        shutdown: Some(shutdown_tx),
    })
}

async fn authenticate(session: &mut Handle<AcceptAll>, ssh: &SshConfig) -> Result<()> {
    if let Some(path) = &ssh.private_key_path {
        let key = load_secret_key(path, ssh.passphrase.as_deref())
            .map_err(|e| MyDbError::Ssh(format!("cannot load ssh key: {e}")))?;
        let hash = PrivateKeyWithHashAlg::new(Arc::new(key), None);
        let result = session
            .authenticate_publickey(&ssh.user, hash)
            .await
            .map_err(|e| MyDbError::Ssh(e.to_string()))?;
        if result.success() {
            return Ok(());
        }
        return Err(MyDbError::Ssh(
            "public-key authentication was rejected".into(),
        ));
    }
    if let Some(password) = &ssh.password {
        let result = session
            .authenticate_password(&ssh.user, password)
            .await
            .map_err(|e| MyDbError::Ssh(e.to_string()))?;
        if result.success() {
            return Ok(());
        }
        return Err(MyDbError::Ssh(
            "password authentication was rejected".into(),
        ));
    }
    Err(MyDbError::Ssh(
        "ssh config needs a private_key_path or password".into(),
    ))
}

async fn pump(
    handle: Arc<Handle<AcceptAll>>,
    mut stream: tokio::net::TcpStream,
    dest_host: &str,
    dest_port: u16,
) -> Result<()> {
    let mut channel = handle
        .channel_open_direct_tcpip(dest_host, u32::from(dest_port), "127.0.0.1", 0)
        .await
        .map_err(|e| MyDbError::Ssh(e.to_string()))?;
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        tokio::select! {
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { ref data }) => {
                    stream
                        .write_all(data)
                        .await
                        .map_err(|e| MyDbError::Ssh(e.to_string()))?;
                }
                Some(ChannelMsg::Eof) | None => break,
                Some(ChannelMsg::Close) => break,
                _ => {}
            },
            read = stream.read(&mut buf) => {
                let n = read.map_err(|e| MyDbError::Ssh(e.to_string()))?;
                if n == 0 {
                    let _ = channel.eof().await;
                    break;
                }
                channel
                    .data(&buf[..n])
                    .await
                    .map_err(|e| MyDbError::Ssh(e.to_string()))?;
            }
        }
    }
    Ok(())
}

/// Destination host/port for a profile (URL wins over discrete fields).
pub fn destination(profile: &crate::conn::ConnectionProfile) -> Result<(String, u16)> {
    if let Some(url) = &profile.url {
        let parsed = url::Url::parse(url)
            .map_err(|e| MyDbError::InvalidValue(format!("invalid connection URL: {e}")))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| MyDbError::InvalidValue("connection URL has no host".into()))?
            .to_string();
        let port = parsed.port().unwrap_or_else(|| profile.kind.default_port());
        return Ok((host, port));
    }
    let host = profile.host.clone().ok_or_else(|| {
        MyDbError::InvalidValue(format!("connection '{}' has no host", profile.name))
    })?;
    let port = profile.port.unwrap_or_else(|| profile.kind.default_port());
    Ok((host, port))
}

/// Rewrite `profile` so sqlx connects through `local_addr`.
pub fn rewrite_through_local(
    mut profile: crate::conn::ConnectionProfile,
    local_addr: SocketAddr,
) -> crate::conn::ConnectionProfile {
    if let Some(url) = profile.url.take() {
        if let Ok(mut parsed) = url::Url::parse(&url) {
            let _ = parsed.set_host(Some("127.0.0.1"));
            let _ = parsed.set_port(Some(local_addr.port()));
            profile.url = Some(parsed.to_string());
        } else {
            profile.url = Some(url);
        }
    }
    profile.host = Some("127.0.0.1".into());
    profile.port = Some(local_addr.port());
    profile.ssh = None;
    profile
}
