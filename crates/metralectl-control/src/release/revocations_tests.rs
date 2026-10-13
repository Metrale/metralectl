// SPDX-License-Identifier: MIT OR Apache-2.0

//! Accepting a revocation list: the bundle's list, signed by a trusted test
//! signer, with each refusal from one change.

use super::*;
use crate::platform::canonical::digest;
use crate::platform::revocation::{Revocation, RevocationReason};
use crate::platform::scalars::UnixMs;
use crate::release::testing::{TestSigner, fixture, signer};

/// The bundle's list: sequence 2, issued at 1_791_000_600_000, revoking a
/// receipt and a release manifest.
fn list() -> RevocationList {
    fixture!(
        RevocationList,
        "fixtures/revocation-list/valid/two-revoked.json"
    )
}

const ISSUED: u64 = 1_791_000_600_000;
const HOUR_MS: u64 = 3_600_000;

fn policy() -> ListPolicy {
    ListPolicy {
        max_age_ms: HOUR_MS,
        held_sequence: NonZeroU64::new(1),
        now_unix_ms: ISSUED + 60_000,
    }
}

fn run(list: &RevocationList, by: &TestSigner, p: ListPolicy) -> Result<Accepted, ListRefused> {
    let sig = by.sign(SignedKind::RevocationList, &digest(list).expect("integers"));
    accept(&signer().anchor(), list, &sig, p)
}

/// A current, newer list from a trusted signer becomes the revoked set.
#[test]
fn a_signed_current_newer_list_is_accepted_into_the_revoked_set() {
    let l = list();
    let a = run(&l, &signer(), policy()).expect("accepted");
    assert_eq!(a.sequence, l.sequence);
    let manifest = "a78cf072e759fb43522f696d040a436025ef7f0e5f1eb37018df163cb8fb4e28";
    assert!(
        a.revoked
            .manifests
            .iter()
            .any(|d| d.to_string() == manifest)
    );
    assert_eq!(a.revoked.receipts.len(), 1);
    assert!(a.revoked.signers.is_empty());
    // The same sequence again is not a rollback.
    let same = ListPolicy {
        held_sequence: Some(l.sequence),
        ..policy()
    };
    assert!(run(&l, &signer(), same).is_ok());
}

/// Sabotage: building the set without verifying the list's signature lets an
/// untrusted signer, or a signature over another list, revoke anything.
#[test]
fn an_unsigned_list_is_refused() {
    let l = list();
    let untrusted = run(&l, &TestSigner::from_seed(9), policy());
    assert!(matches!(untrusted, Err(ListRefused::Unsigned(_))));
    let mut edited = l.clone();
    edited.revoked.pop();
    let sig = signer().sign(SignedKind::RevocationList, &digest(&l).expect("integers"));
    let other = accept(&signer().anchor(), &edited, &sig, policy());
    assert!(matches!(other, Err(ListRefused::Unsigned(_))));
    // A release-manifest signature over the list's digest is not a list's.
    let as_release = signer().sign(SignedKind::ReleaseManifest, &digest(&l).expect("integers"));
    let mut relabelled = as_release.clone();
    relabelled.kind = SignedKind::RevocationList;
    for s in [as_release, relabelled] {
        let r = accept(&signer().anchor(), &l, &s, policy());
        assert!(matches!(r, Err(ListRefused::Unsigned(_))));
    }
}

/// Sabotage: skipping the check accepts a list in which a signer revokes
/// itself, which is how a stolen key would try to look current.
#[test]
fn a_list_that_revokes_its_own_signer_is_refused() {
    let mut l = list();
    l.revoked.push(Revocation {
        subject: RevokedSubject::Signer(signer().id()),
        reason: RevocationReason::KeyCompromised,
        revoked_at_unix_ms: UnixMs::new(ISSUED).expect("in range"),
    });
    assert_eq!(
        run(&l, &signer(), policy()),
        Err(ListRefused::SignedByARevokedSigner(signer().id()))
    );
}

/// Sabotage: skipping the sequence check takes back a revocation by
/// replaying an older list.
#[test]
fn an_older_list_than_the_one_held_is_refused() {
    let held = ListPolicy {
        held_sequence: NonZeroU64::new(3),
        ..policy()
    };
    assert!(matches!(
        run(&list(), &signer(), held),
        Err(ListRefused::Rollback { .. })
    ));
}

/// Sabotage: skipping the age check keeps a list forever, so a revocation
/// issued after it never arrives and nothing notices.
#[test]
fn a_list_older_than_the_maximum_age_is_refused() {
    let late = ListPolicy {
        now_unix_ms: ISSUED + HOUR_MS + 1,
        ..policy()
    };
    assert_eq!(
        run(&list(), &signer(), late),
        Err(ListRefused::Stale {
            age_ms: HOUR_MS + 1,
            max_age_ms: HOUR_MS
        })
    );
    let edge = ListPolicy {
        now_unix_ms: ISSUED + HOUR_MS,
        ..policy()
    };
    assert!(run(&list(), &signer(), edge).is_ok());
}
