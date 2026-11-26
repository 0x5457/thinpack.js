#[cfg(unix)]
mod unix {
    //! The cluster primary's spawn op.
    //!
    //! The worker process itself — its pid, stdio pipes, wait, kill and IPC channel — lives
    //! in the shared `thinpack-child` layer. This module only adds the cluster-specific
    //! environment and started id.

    use std::io;

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use thinpack_child::{ChildError, Spawn, Stdio};

    /// The descriptor the child side of the channel is moved to before `exec`.
    const CHANNEL_FD: i32 = 3;

    pub(crate) const OPS: &[OpDecl] = &[
        op_cluster_spawn(),
        op_cluster_clear_env(),
        op_cluster_allocate_port(),
    ];

    #[expect(
        clippy::needless_pass_by_value,
        reason = "`io::Error` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    fn io_error(error: io::Error) -> JsErrorBox {
        let errno = error.raw_os_error().unwrap_or(libc::EIO);
        JsErrorBox::new("Error", format!("-{errno}\0{error}"))
    }

    /// A spawn failure in the errno form `UVException` is raised from.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    fn spawn_error(error: ChildError) -> JsErrorBox {
        JsErrorBox::new("Error", error.to_uv_wire())
    }

    #[op2]
    #[string]
    fn op_cluster_spawn(
        #[string] argv: &str,
        #[string] env: &str,
        #[string] cwd: &str,
        silent: bool,
        worker_id: u32,
    ) -> Result<String, JsErrorBox> {
        let request = Spawn::parse(argv, env, cwd)
            .map_err(spawn_error)?
            .with_env("NODE_UNIQUE_ID", worker_id);
        let stdio = if silent {
            Stdio::PIPED
        } else {
            Stdio::INHERITED
        };
        let spawned = thinpack_child::channel::spawn_with_channel(request, stdio, CHANNEL_FD)
            .map_err(spawn_error)?;
        Ok(format!(
            "{}\0{}\0{}",
            spawned.spawned.id, spawned.spawned.pid, spawned.channel
        ))
    }

    #[op2(fast)]
    fn op_cluster_clear_env() {
        // SAFETY: a cluster worker calls this once during startup, before user code can spawn
        // children. These variables belong to the worker bootstrap and are not read afterward.
        unsafe {
            std::env::remove_var("NODE_UNIQUE_ID");
            std::env::remove_var("NODE_CHANNEL_FD");
            std::env::remove_var("NODE_CHANNEL_PORT");
            std::env::remove_var("NODE_CHANNEL_SERIALIZATION_MODE");
        }
    }

    #[op2(fast)]
    fn op_cluster_allocate_port(#[string] host: &str) -> Result<u16, JsErrorBox> {
        let listener = std::net::TcpListener::bind((host, 0)).map_err(io_error)?;
        listener
            .local_addr()
            .map(|address| address.port())
            .map_err(io_error)
    }
}

#[cfg(unix)]
pub(crate) use unix::OPS;

#[cfg(windows)]
mod windows {
    //! The cluster primary's spawn op. The channel itself is the shared loopback channel;
    //! this module only adds the cluster-specific environment and worker id.

    use deno_core::{OpDecl, op2};
    use deno_error::JsErrorBox;
    use thinpack_child::{ChildError, Spawn, Stdio};

    #[op2]
    #[string]
    fn op_cluster_spawn(
        #[string] argv: &str,
        #[string] env: &str,
        #[string] cwd: &str,
        silent: bool,
        worker_id: u32,
    ) -> Result<String, JsErrorBox> {
        let request = Spawn::parse(argv, env, cwd)
            .map_err(spawn_error)?
            .with_env("NODE_UNIQUE_ID", worker_id);
        let stdio = if silent {
            Stdio::PIPED
        } else {
            Stdio::INHERITED
        };
        let spawned =
            thinpack_child::channel::spawn_with_channel(request, stdio, 3).map_err(spawn_error)?;
        Ok(format!(
            "{}\0{}\0{}",
            spawned.spawned.id, spawned.spawned.pid, spawned.channel
        ))
    }

    pub(crate) const OPS: &[OpDecl] = &[
        op_cluster_spawn(),
        op_cluster_clear_env(),
        op_cluster_allocate_port(),
    ];

    #[expect(
        clippy::needless_pass_by_value,
        reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    fn spawn_error(error: ChildError) -> JsErrorBox {
        JsErrorBox::new("Error", error.to_uv_wire())
    }

    fn io_error(error: std::io::Error) -> JsErrorBox {
        let errno = deno_core::uv_compat::io_error_to_uv(&error);
        JsErrorBox::new("Error", format!("{errno}\0{error}"))
    }

    #[op2(fast)]
    fn op_cluster_clear_env() {
        // SAFETY: called once during worker startup, before user code can spawn.
        unsafe {
            std::env::remove_var("NODE_UNIQUE_ID");
            std::env::remove_var("NODE_CHANNEL_PORT");
            std::env::remove_var("NODE_CHANNEL_FD");
            std::env::remove_var("NODE_CHANNEL_SERIALIZATION_MODE");
        }
    }

    #[op2(fast)]
    fn op_cluster_allocate_port(#[string] host: &str) -> Result<u16, JsErrorBox> {
        let listener = std::net::TcpListener::bind((host, 0)).map_err(io_error)?;
        listener
            .local_addr()
            .map(|address| address.port())
            .map_err(io_error)
    }
}

#[cfg(windows)]
pub(crate) use windows::OPS;
