//! Crate `cs2`: Orquestación y gestión de servidores de Counter-Strike 2, MatchZy y AWS EC2.

pub mod aws;
pub mod matchzy;
pub mod server_manager;
pub mod ssh;

pub use aws::{AwsError, Ec2Client, InstanceState};
pub use matchzy::{GeneratedMatchZy, MatchConfig, MatchParams};
pub use server_manager::{ActiveMatch, Cs2ServerManager, PORT_POOL};
pub use ssh::{execute_remote_command, resolve_ssh_key, wait_for_ssh_ready, SshError};
