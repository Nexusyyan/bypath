pub mod protocol;
pub mod transport;

pub use protocol::{
    BackendMode, ConfigSnapshot, DaemonStatus, DoctorReport, IpcRequest, IpcResponse,
    NamedProxyProbe, ProcessEntry, DEFAULT_PIPE_NAME,
};
pub use transport::{IpcClient, IpcHandler, IpcServer};
