// SPDX-License-Identifier: MIT OR Apache-2.0

//! The reconciler against a model of the launcher.
//!
//! [`World`] holds what runs and applies a step the way the launcher does: a
//! launch adds the recipe's workload, a stop removes EVERY workload of the
//! recipe. The scenario tests drive [`decide`] against it with no memory
//! between calls, which is the property under test.

use super::contract::*;
use super::*;
use metralectl_core::hardware::HardwareClass;
use metralectl_protocol::RecipeId;

const GB10: &str = "NVIDIA GB10";
const MODEL: &str = "Org/Model-A";

fn rid(s: &str) -> RecipeId {
    RecipeId::parse(s).expect("valid id")
}

fn name(s: &str) -> DeploymentName {
    DeploymentName::parse(s).expect("valid name")
}

fn generation(n: u64) -> Generation {
    Generation::new(n).expect("non-zero")
}

fn owner(deployment: &str, g: u64) -> Option<Owner> {
    Some(Owner {
        deployment: name(deployment),
        generation: generation(g),
    })
}

fn run(recipe: &str, g: u64) -> Desired {
    Desired {
        deployment: name("chat"),
        generation: generation(g),
        intent: Intent::Run {
            recipe: rid(recipe),
            model: MODEL.into(),
            placement: Placement::HardwareClass(HardwareClass::parse("gb10").expect("valid")),
        },
    }
}

fn stopped(g: u64) -> Desired {
    Desired {
        deployment: name("chat"),
        generation: generation(g),
        intent: Intent::Stopped,
    }
}

fn inst(recipe: &str, who: Option<Owner>) -> Instance {
    Instance {
        recipe: rid(recipe),
        owner: who,
    }
}

/// What runs on the node, changed only by carrying out steps.
struct World {
    instances: Vec<Instance>,
    accelerator: &'static str,
    launches: u32,
    stops: u32,
    /// The next launch fails without starting anything.
    fail_next_launch: bool,
    /// The next stop fails without stopping anything.
    fail_next_stop: bool,
}

impl World {
    fn new(instances: Vec<Instance>) -> Self {
        Self {
            instances,
            accelerator: GB10,
            launches: 0,
            stops: 0,
            fail_next_launch: false,
            fail_next_stop: false,
        }
    }

    fn decide(&self, desired: &Desired) -> Step {
        decide(
            desired,
            &Observed {
                instances: &self.instances,
                accelerator: self.accelerator,
                recipe_model: Some(MODEL),
            },
        )
    }

    /// Carry a step out as the launcher would.
    fn apply(&mut self, desired: &Desired, step: &Step) {
        match step {
            Step::Launch { recipe } => {
                self.launches += 1;
                if std::mem::take(&mut self.fail_next_launch) {
                    return;
                }
                self.instances.push(Instance {
                    recipe: recipe.clone(),
                    owner: Some(Owner {
                        deployment: desired.deployment.clone(),
                        generation: desired.generation,
                    }),
                });
            }
            Step::Stop { recipe } => {
                self.stops += 1;
                if std::mem::take(&mut self.fail_next_stop) {
                    return;
                }
                self.instances.retain(|i| i.recipe != *recipe);
            }
            Step::Converged | Step::Refuse(_) => {}
        }
    }

    /// Decide and apply until nothing is left to do; the final answer.
    fn converge(&mut self, desired: &Desired) -> Step {
        for _ in 0..8 {
            let step = self.decide(desired);
            if matches!(step, Step::Converged | Step::Refuse(_)) {
                return step;
            }
            self.apply(desired, &step);
        }
        panic!("did not converge in 8 steps: {:?}", self.instances);
    }

    fn running(&self, recipe: &str) -> usize {
        self.instances
            .iter()
            .filter(|i| i.recipe == rid(recipe))
            .count()
    }
}

#[test]
fn a_fresh_deployment_is_launched_once() {
    let mut w = World::new(vec![]);
    assert_eq!(
        w.decide(&run("r-a", 1)),
        Step::Launch { recipe: rid("r-a") }
    );
    assert_eq!(w.converge(&run("r-a", 1)), Step::Converged);
    assert_eq!((w.launches, w.running("r-a")), (1, 1));
}

#[test]
fn a_repeated_apply_launches_nothing_more() {
    let mut w = World::new(vec![]);
    w.converge(&run("r-a", 1));
    for _ in 0..3 {
        assert_eq!(w.converge(&run("r-a", 1)), Step::Converged);
    }
    assert_eq!((w.launches, w.running("r-a")), (1, 1));
}

#[test]
fn an_apply_resent_after_a_lost_acknowledgement_finds_the_first() {
    let mut w = World::new(vec![]);
    let desired = run("r-a", 1);
    let step = w.decide(&desired);
    w.apply(&desired, &step);
    // The acknowledgement never reached the plane, which sends the same
    // desired state again.
    assert_eq!(w.decide(&desired), Step::Converged);
    assert_eq!((w.launches, w.running("r-a")), (1, 1));
}

#[test]
fn a_restarted_host_observes_what_it_left_and_starts_nothing() {
    // The workload outlives the host; a new host has only the launcher's
    // account, which already carries the owner.
    let w = World::new(vec![inst("r-a", owner("chat", 1))]);
    assert_eq!(w.decide(&run("r-a", 1)), Step::Converged);
    assert_eq!(w.decide(&stopped(1)), Step::Stop { recipe: rid("r-a") });
}

#[test]
fn a_failed_launch_is_retried_and_never_doubled() {
    let mut w = World::new(vec![]);
    w.fail_next_launch = true;
    assert_eq!(w.converge(&run("r-a", 1)), Step::Converged);
    assert_eq!((w.launches, w.running("r-a")), (2, 1));
}

#[test]
fn a_failed_stop_is_stopped_again_not_launched_over() {
    let mut w = World::new(vec![inst("r-old", owner("chat", 1))]);
    w.fail_next_stop = true;
    let desired = run("r-new", 2);
    let first = w.decide(&desired);
    assert_eq!(
        first,
        Step::Stop {
            recipe: rid("r-old")
        }
    );
    w.apply(&desired, &first);
    assert_eq!(
        w.decide(&desired),
        Step::Stop {
            recipe: rid("r-old")
        },
        "the old generation still runs, so it is stopped again"
    );
    assert_eq!(w.converge(&desired), Step::Converged);
    assert_eq!(
        (w.running("r-old"), w.running("r-new"), w.launches),
        (0, 1, 1)
    );
}

#[test]
fn a_new_generation_replaces_only_its_own_older_one() {
    let mut w = World::new(vec![
        inst("r-a", owner("chat", 1)),
        inst("r-other", owner("embed", 4)),
        inst("r-hand", None),
    ]);
    assert_eq!(w.converge(&run("r-b", 2)), Step::Converged);
    assert_eq!((w.running("r-a"), w.running("r-b")), (0, 1));
    assert_eq!((w.running("r-other"), w.running("r-hand")), (1, 1));
}

#[test]
fn a_new_generation_of_the_same_recipe_is_relaunched_for_it() {
    let mut w = World::new(vec![inst("r-a", owner("chat", 1))]);
    assert_eq!(w.converge(&run("r-a", 2)), Step::Converged);
    assert_eq!(w.instances, vec![inst("r-a", owner("chat", 2))]);
}

#[test]
fn an_older_generation_than_the_one_running_is_refused_not_applied() {
    let mut w = World::new(vec![inst("r-b", owner("chat", 5))]);
    let want = Step::Refuse(Refusal::StaleGeneration {
        running: generation(5),
    });
    assert_eq!(w.converge(&run("r-a", 3)), want);
    assert_eq!(w.converge(&stopped(3)), want);
    assert_eq!((w.launches, w.stops, w.running("r-b")), (0, 0, 1));
}

#[test]
fn stop_stops_only_this_deployment() {
    let mut w = World::new(vec![
        inst("r-a", owner("chat", 2)),
        inst("r-other", owner("embed", 1)),
        inst("r-hand", None),
    ]);
    assert_eq!(w.converge(&stopped(2)), Step::Converged);
    assert_eq!(
        (w.running("r-a"), w.running("r-other"), w.running("r-hand")),
        (0, 1, 1)
    );
    assert_eq!(w.converge(&stopped(2)), Step::Converged, "a repeated stop");
    assert_eq!(w.stops, 1);
}

#[test]
fn a_recipe_held_by_another_workload_is_busy_not_launched_or_adopted() {
    let mut w = World::new(vec![inst("r-a", None)]);
    let step = w.converge(&run("r-a", 1));
    assert_eq!(
        step,
        Step::Refuse(Refusal::Busy {
            recipe: rid("r-a"),
            owner: None
        })
    );
    assert_eq!((w.launches, w.stops, w.running("r-a")), (0, 0, 1));
}

#[test]
fn a_stop_that_would_catch_a_foreign_workload_of_the_recipe_is_refused() {
    // A stop is by recipe; the hand-started rank of the same recipe would go
    // with this deployment's workload.
    let mut w = World::new(vec![inst("r-a", owner("chat", 1)), inst("r-a", None)]);
    assert!(matches!(
        w.converge(&stopped(1)),
        Step::Refuse(Refusal::Busy { .. })
    ));
    assert!(matches!(
        w.converge(&run("r-b", 2)),
        Step::Refuse(Refusal::Busy { .. })
    ));
    assert_eq!((w.stops, w.running("r-a")), (0, 2));
}

#[test]
fn incompatible_hardware_is_refused_and_the_running_generation_kept() {
    let mut w = World::new(vec![inst("r-a", owner("chat", 1))]);
    w.accelerator = "NVIDIA H100 80GB HBM3";
    let step = w.converge(&run("r-b", 2));
    assert!(
        matches!(&step, Step::Refuse(Refusal::HardwareIncompatible { reason }) if reason.contains("gb10")),
        "{step:?}"
    );
    assert_eq!((w.stops, w.running("r-a")), (0, 1));
}

#[test]
fn an_explicitly_placed_node_needs_no_class() {
    let mut w = World::new(vec![]);
    w.accelerator = "";
    let mut desired = run("r-a", 1);
    if let Intent::Run { placement, .. } = &mut desired.intent {
        *placement = Placement::ThisNode;
    }
    assert_eq!(w.decide(&desired), Step::Launch { recipe: rid("r-a") });
}

#[test]
fn a_release_this_node_cannot_match_is_refused_and_the_old_generation_kept() {
    let instances = vec![inst("r-a", owner("chat", 1))];
    for recipe_model in [None, Some("Org/Model-B")] {
        let step = decide(
            &run("r-b", 2),
            &Observed {
                instances: &instances,
                accelerator: GB10,
                recipe_model,
            },
        );
        assert_eq!(step.word(), Some("release_unverified"), "{recipe_model:?}");
    }
}

#[test]
fn one_generation_with_two_meanings_is_a_conflict() {
    let w = World::new(vec![inst("r-a", owner("chat", 3))]);
    assert_eq!(
        w.decide(&run("r-b", 3)),
        Step::Refuse(Refusal::IdempotencyConflict {
            running: rid("r-a")
        })
    );
}

#[test]
fn steps_become_only_the_closed_launch_and_stop_verbs_without_settings() {
    let a = rid("r-a");
    assert_eq!(
        Step::Launch { recipe: a.clone() }.request(),
        Some(ControlReq::Launch {
            recipe: a.clone(),
            settings: BTreeMap::new()
        })
    );
    assert_eq!(
        Step::Stop { recipe: a.clone() }.request(),
        Some(ControlReq::Stop { recipe: a })
    );
    assert_eq!(Step::Converged.request(), None);
    assert_eq!(
        Step::Refuse(Refusal::StaleGeneration {
            running: generation(1)
        })
        .request(),
        None
    );
}

#[test]
fn contract_words_are_the_platform_vocabulary() {
    // deployment-action-result.v1 `refusal_reason`: every word used here must
    // be one of the platform's, spelled as it spells it.
    const PLATFORM: [&str; 7] = [
        "stale_generation",
        "idempotency_conflict",
        "release_unverified",
        "receipt_missing",
        "hardware_incompatible",
        "not_here",
        "busy",
    ];
    let ours = [
        Refusal::StaleGeneration {
            running: generation(1),
        },
        Refusal::IdempotencyConflict { running: rid("r") },
        Refusal::ReleaseUnverified { reason: "x".into() },
        Refusal::HardwareIncompatible { reason: "x".into() },
        Refusal::Busy {
            recipe: rid("r"),
            owner: None,
        },
    ];
    for r in &ours {
        assert!(PLATFORM.contains(&r.word()), "{}", r.word());
    }
}

#[test]
fn names_and_generations_refuse_what_the_contract_refuses() {
    assert!(DeploymentName::parse("chat-1").is_ok());
    for bad in ["", "Chat", "-chat", "chat_1"] {
        assert!(DeploymentName::parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(Generation::new(0), None);
    assert_eq!(Generation::new(7).map(Generation::get), Some(7));
}

impl Step {
    /// The contract word of a refusal, for assertions.
    fn word(&self) -> Option<&'static str> {
        match self {
            Self::Refuse(r) => Some(r.word()),
            _ => None,
        }
    }
}
