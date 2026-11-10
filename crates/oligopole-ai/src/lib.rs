//! `oligopole-ai`: Asymmetric `ActorObservationView`, NPV-ranked `JointActionTuple` candidate generator,
//! `PersonaBiasVector` log-odds modulator, `System0HeuristicPolicy`, and `127.0.0.1:19509` `TCP_NODELAY`
//! length-prefixed `msgpack` client for the local RTX 5090 `Clef-Flash` 9B sidecar.

pub mod candidate;
pub mod client;
pub mod error;
pub mod observation;
pub mod persona;
pub mod protocol;
pub mod system0;

pub use candidate::{CandidateOption, generate_joint_action_candidates};
pub use client::ClefFlashTcpClient;
pub use error::AiError;
pub use observation::ActorObservationView;
pub use persona::{PersonaBiasVector, modulate_and_sample_choice, seeded_actor_rng};
pub use protocol::{
    ClefQuestionDistribution, ClefQuestionType, ClefTypedQuestion, ClefWorkerRequest,
    ClefWorkerResponse, DEFAULT_CLEF_WORKER_ADDR, HandshakeVerification,
};
pub use system0::{DecisionPolicy, System0HeuristicPolicy};
