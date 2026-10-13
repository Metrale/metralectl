// SPDX-License-Identifier: MIT OR Apache-2.0

//! The verifier against the platform's published test signer
//! (`v1/vectors/release-signer.json`) and its real signature over the
//! bundle's release manifest (`fixtures/document-signature/valid/…`): it must
//! accept that signature and refuse it after any change.

use super::*;
use crate::platform::release::ReleaseManifest;
use crate::release::testing::{TestSigner, fixture};

/// The vector's principal: the file is the principal plus a `purpose` note.
fn published() -> RegistryPrincipal {
    let mut v: serde_json::Value = fixture!(serde_json::Value, "vectors/release-signer.json");
    v.as_object_mut().expect("an object").remove("purpose");
    serde_json::from_value(v).expect("a principal")
}

fn signature() -> DocumentSignature {
    fixture!(
        DocumentSignature,
        "fixtures/document-signature/valid/release-manifest.json"
    )
}

fn manifest_digest() -> Sha256Digest {
    let m: ReleaseManifest = fixture!(
        ReleaseManifest,
        "fixtures/release-manifest/valid/gpt-oss-20b-gb10.json"
    );
    crate::platform::canonical::digest(&m).expect("integers")
}

fn anchor() -> RegistryPrincipals {
    RegistryPrincipals::new(&[published()]).expect("the published signer")
}

fn check(sig: &DocumentSignature) -> Result<SignerId, String> {
    anchor().verify(SignedKind::ReleaseManifest, &manifest_digest(), sig)
}

/// The published signature verifies under the published keys, and the
/// signer's identity is the one its keys derive exactly as the platform
/// derives it. Sabotage: dropping the domain prefix from the signed string,
/// or deriving the identity without its domain, fails this.
#[test]
fn the_published_test_signer_verifies_its_signature_over_the_bundle_manifest() {
    assert_eq!(
        manifest_digest().to_string(),
        "a78cf072e759fb43522f696d040a436025ef7f0e5f1eb37018df163cb8fb4e28"
    );
    assert_eq!(published().signing.node_id(), published().node_id);
    assert_eq!(check(&signature()), Ok(published().node_id));
}

fn flip<const N: usize>(b: &HexBytes<N>, at: usize) -> HexBytes<N> {
    let mut raw = *b.as_bytes();
    raw[at] ^= 0x01;
    HexBytes::new(raw)
}

/// Any change to the signature, or to what it is checked against, is
/// refused. Sabotage: checking only one half accepts the signature whose
/// other half is broken (both directions are here).
#[test]
fn any_change_to_the_published_signature_is_refused() {
    let mut s = signature();
    s.signature.ed25519 = flip(&s.signature.ed25519, 5);
    assert_eq!(check(&s), Err("the Ed25519 half does not verify".into()));

    let mut s = signature();
    s.signature.ml_dsa_65 = flip(&s.signature.ml_dsa_65, 100);
    assert_eq!(check(&s), Err("the ML-DSA-65 half does not verify".into()));

    // Each half valid on its own, from two different signings: still refused.
    let other = TestSigner::from_seed(9).sign(SignedKind::ReleaseManifest, &manifest_digest());
    let mut s = signature();
    s.signature.ml_dsa_65 = other.signature.ml_dsa_65.clone();
    assert!(check(&s).is_err());
    let mut s = signature();
    s.signature.ed25519 = other.signature.ed25519;
    assert!(check(&s).is_err());

    // Another document, another kind, an untrusted signer.
    let other_doc = crate::platform::canonical::sha256(b"another manifest");
    assert!(
        anchor()
            .verify(SignedKind::ReleaseManifest, &other_doc, &signature())
            .is_err()
    );
    // A relabelled document is named as such, before any key is tried.
    let err = anchor()
        .verify(SignedKind::ReleaseManifest, &other_doc, &signature())
        .expect_err("another document");
    assert!(err.starts_with("the signature is over "), "{err}");
    let mut s = signature();
    s.document = other_doc;
    assert!(
        anchor()
            .verify(SignedKind::ReleaseManifest, &other_doc, &s)
            .is_err(),
        "the signature does not cover a relabelled document"
    );
    assert!(
        anchor()
            .verify(SignedKind::RevocationList, &manifest_digest(), &signature())
            .is_err()
    );
    let mut s = signature();
    s.kind = SignedKind::RevocationList;
    assert!(
        anchor()
            .verify(SignedKind::RevocationList, &manifest_digest(), &s)
            .is_err(),
        "the release domain does not verify as a revocation list"
    );
    let mut s = signature();
    s.signer = TestSigner::from_seed(9).id();
    assert!(check(&s).expect_err("untrusted").contains("not a trusted"));
}

/// Trust is only what the host names: no principal, a principal whose
/// identity is not its keys', and one listed twice are each refused.
/// Sabotage: a default anchor, or skipping the identity check, lets a
/// principal claim another's identity.
#[test]
fn the_anchor_is_explicit_and_each_identity_is_its_keys() {
    assert!(RegistryPrincipals::new(&[]).is_err());
    let mut impostor = TestSigner::from_seed(9).principal();
    impostor.node_id = published().node_id;
    let err = RegistryPrincipals::new(&[impostor]).expect_err("an identity it does not own");
    assert!(err.contains("has keys of"), "{err}");
    assert!(RegistryPrincipals::new(&[published(), published()]).is_err());
    let two = RegistryPrincipals::new(&[published(), TestSigner::from_seed(9).principal()])
        .expect("two principals");
    assert!(
        two.verify(
            SignedKind::ReleaseManifest,
            &manifest_digest(),
            &signature()
        )
        .is_ok()
    );
}

/// Writers emit lower case; readers accept either.
#[test]
fn a_signature_round_trips_in_lower_case_and_reads_upper_case() {
    let s = signature();
    let json = serde_json::to_value(&s).expect("serializes");
    let ed = json["signature"]["ed25519"].as_str().expect("hex");
    assert_eq!(ed, ed.to_lowercase());
    let mut upper = json.clone();
    upper["signature"]["ed25519"] = ed.to_uppercase().into();
    let back: DocumentSignature = serde_json::from_value(upper).expect("either case");
    assert_eq!(back, s);
}
