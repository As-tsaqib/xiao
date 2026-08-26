mod android;
mod capabilities;
mod dependency;
mod environment;
mod execution;
pub mod host;

pub use android::{AndroidBroker, AndroidOperation, AndroidOutcome, SystemAndroidBroker};
pub use capabilities::trusted_package_for_binary;
pub use capabilities::{Capability, CapabilityRegistry, CapabilityResolution, CapabilityStatus};
pub use dependency::{
    validate_binary, validate_package, DependencyResolution, DependencyResolver, PackageBackend,
    PackageCandidate, TermuxPackageBackend, TermuxRepositoryBackend, TrustedPackageRepository,
};
pub use environment::{
    EnvironmentProbe, ExecutionBackend, HostProbe, RealHostProbe, RuntimeEnvironment, RuntimeState,
    SelinuxState, TermuxEnvironment,
};
pub use execution::{
    validate_terminal_request, CommandOutcome, ExecutionPurpose, ProcessExecutor, TermuxCommand,
    TermuxExecutor,
};
