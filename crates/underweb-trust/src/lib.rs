//! Trust: who published a package, and who vouches for it.
//!
//! Everything rests on [`Statement`]s: small JSON documents signed with an
//! Ed25519 key. A publisher signs a *release* statement covering the hashes
//! of a version's archives; anyone can sign a *vouch* (used it, reviewed it)
//! or a *warning*. Statements are verified locally, the same way whatever
//! network they came from.
//!
//! [`TrustAdapter`]s connect identities to keys and find statements:
//! ATProto accounts (`underweb-trust-atproto`), signed lists ([`lists`]),
//! and domains (a key at `https://<domain>/.well-known/underweb/key`). The
//! [`TrustStore`] holds who the user follows and pinned keys; [`evaluate`]
//! turns the statements about an archive into a [`Verdict`], which decides
//! whether a package's declared permissions are granted without asking.
//! Trust never turns the sandbox off.

mod adapter;
mod keys;
pub mod lists;
mod policy;
mod statement;
mod store;

pub use adapter::{Identity, TrustAdapter};
pub use keys::{KeyPair, check_key, decode_key};
pub use policy::{Signer, Verdict, evaluate};
pub use statement::{Kind, Level, Statement};
pub use store::{Follow, PolicySettings, TrustStore};
