use nodeharbor_core::{
    evaluate, qualify, validate_policy, HealthWindow, Observation, Policy, Resources,
    ScheduleWindow,
};
fn host() -> Observation {
    Observation {
        idle_seconds: Some(1800),
        on_battery: Some(false),
        battery_percent: Some(90),
        weekday: 0,
        minute: 600,
        resources: Resources {
            cpus: 8,
            memory_mib: 16384,
            disk_gib: 100,
        },
    }
}
#[test]
fn sharing_is_opt_in() {
    assert!(!evaluate(&Policy::default(), &host()).allowed);
}
#[test]
fn all_enabled_rules_must_permit_sharing() {
    let mut policy = Policy {
        enabled: true,
        idle_only: true,
        ..Default::default()
    };
    let mut observation = host();
    assert!(evaluate(&policy, &observation).allowed);
    observation.on_battery = Some(true);
    assert!(evaluate(&policy, &observation).reason.contains("battery"));
    policy.allow_battery = true;
    assert!(evaluate(&policy, &observation).allowed);
    observation.battery_percent = Some(5);
    assert!(!evaluate(&policy, &observation).allowed);
    observation.battery_percent = Some(90);
    observation.idle_seconds = Some(0);
    assert!(!evaluate(&policy, &observation).allowed);
}
#[test]
fn unavailable_idle_sensor_never_silently_broadens_sharing() {
    let p = Policy {
        enabled: true,
        idle_only: true,
        ..Default::default()
    };
    let h = Observation {
        idle_seconds: None,
        ..host()
    };
    assert!(evaluate(&p, &h).reason.contains("unavailable"));
}
#[test]
fn schedules_include_overnight_windows_and_exclude_other_days() {
    let p = Policy {
        enabled: true,
        schedule_enabled: true,
        schedule: vec![ScheduleWindow {
            days: vec![0],
            start_minute: 1320,
            end_minute: 120,
        }],
        ..Default::default()
    };
    assert!(
        evaluate(
            &p,
            &Observation {
                weekday: 1,
                minute: 60,
                ..host()
            }
        )
        .allowed
    );
    assert!(
        !evaluate(
            &p,
            &Observation {
                weekday: 2,
                minute: 60,
                ..host()
            }
        )
        .allowed
    );
    assert!(
        !evaluate(
            &p,
            &Observation {
                weekday: 1,
                minute: 120,
                ..host()
            }
        )
        .allowed
    );
}
#[test]
fn schedule_end_is_exclusive_and_empty_schedule_is_not_always_on() {
    let p = Policy {
        enabled: true,
        schedule_enabled: true,
        ..Default::default()
    };
    assert!(!evaluate(&p, &host()).allowed);
}
#[test]
fn budgets_leave_capacity_for_the_owner() {
    let mut p = Policy::default();
    p.resources.memory_mib = 16000;
    assert!(validate_policy(&p, &host().resources).is_err());
    p.resources.memory_mib = 4096;
    p.resources.cpus = 0;
    assert!(validate_policy(&p, &host().resources).is_err());
}

#[test]
fn storage_minimum_and_free_space_failures_have_distinct_actionable_messages() {
    let mut policy = Policy::default();
    policy.resources.disk_gib = 14;
    let error = validate_policy(&policy, &host().resources).unwrap_err();
    assert!(
        error.contains("at least 15 GiB") && error.contains("14 GiB"),
        "{error}"
    );
    assert!(
        !error.contains("10 GiB"),
        "A minimum-size error is not a free-space error: {error}"
    );
    policy.resources.disk_gib = 100;
    let mut capacity = host().resources;
    capacity.disk_gib = 27;
    let error = validate_policy(&policy, &capacity).unwrap_err();
    for detail in ["100 GiB", "17 GiB", "10 GiB", "another drive"] {
        assert!(error.contains(detail), "Missing {detail}: {error}");
    }
    assert!(
        !error.contains("at least 15"),
        "100 GiB already exceeds the minimum: {error}"
    );
    capacity.disk_gib = 110;
    assert!(validate_policy(&policy, &capacity).is_ok());
    policy.resources.disk_gib = 15;
    capacity.disk_gib = 25;
    assert!(validate_policy(&policy, &capacity).is_ok());
}
#[test]
fn choosing_no_workload_class_prevents_admission() {
    let p = Policy {
        enabled: true,
        allow_ci: false,
        allow_services: false,
        ..Default::default()
    };
    assert!(!evaluate(&p, &host()).allowed);
}
#[test]
fn qualification_requires_controller_observations_and_rejects_stale_health() {
    let h = HealthWindow {
        healthy_seconds: 600,
        availability: 1.0,
        p95_rtt_ms: 200.0,
        loss: 0.0,
        ready: true,
        age_seconds: 0,
    };
    assert!(qualify(&h).ci);
    assert!(!qualify(&h).services);
    assert!(
        qualify(&HealthWindow {
            healthy_seconds: 86400,
            ..h.clone()
        })
        .services
    );
    assert!(
        !qualify(&HealthWindow {
            age_seconds: 91,
            ..h.clone()
        })
        .ci
    );
    assert!(!qualify(&HealthWindow { loss: 0.1, ..h }).services);
}

#[test]
fn unknown_power_never_bypasses_the_minimum_battery_rule() {
    let p = Policy {
        enabled: true,
        allow_battery: true,
        ..Default::default()
    };
    assert!(
        !evaluate(
            &p,
            &Observation {
                on_battery: None,
                battery_percent: None,
                ..host()
            }
        )
        .allowed
    );
}
