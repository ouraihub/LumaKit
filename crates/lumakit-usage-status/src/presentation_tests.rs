use super::*;

#[test]
fn unknown_is_not_zero() {
    let mut status = ContextStatus::default();
    assert_eq!(summary(&status), "?%/?");
    status.used_tokens = Some(123_456);
    status.context_window = Some(1_100_000);
    status.estimate = true;
    assert_eq!(summary(&status), "~?%/1.1M");
}

#[test]
fn compact_counts_do_not_overstate_boundaries() {
    for (n, expected) in [
        (0, "0"),
        (999, "999"),
        (1_000, "1k"),
        (1_199, "1.1k"),
        (999_999, "999.9k"),
        (1_100_000, "1.1M"),
        (1_000_000_000, "1B"),
    ] {
        assert_eq!(compact(n), expected);
    }
    assert_eq!(compact(u64::MAX), "18446744073.7B");
}

#[test]
fn invalid_values_are_unknown_but_observed_zero_is_real() {
    for p in [f32::NAN, f32::INFINITY, -1., 101.] {
        let status = ContextStatus {
            percent: Some(p),
            context_window: Some(0),
            ..Default::default()
        };
        assert_eq!(summary(&status), "?%/?");
    }
    for amount in [f64::NAN, f64::INFINITY, -0.1] {
        assert_eq!(
            money(&Money {
                currency: "USD".into(),
                amount,
                estimated: false,
                partial: false,
            }),
            None
        );
    }
    assert_eq!(
        money(&Money {
            currency: " ".into(),
            amount: 1.,
            estimated: false,
            partial: false,
        }),
        None
    );
    assert_eq!(
        money(&Money {
            currency: "USD".into(),
            amount: 0.,
            estimated: false,
            partial: false,
        }),
        Some("$0.00".into())
    );
    assert_eq!(
        money(&Money {
            currency: "EUR".into(),
            amount: 0.00001,
            estimated: false,
            partial: false,
        }),
        Some("EUR <0.0001".into())
    );
    let status = ContextStatus {
        percent: Some(0.),
        used_tokens: Some(0),
        cumulative_tokens: Some(0),
        ..Default::default()
    };
    assert_eq!(summary(&status), "0.0%/?");
}

#[test]
fn cost_estimates_and_partial_coverage_are_distinct_from_known_zero() {
    let value = |estimated, partial| Money {
        currency: "USD".into(),
        amount: 1.25,
        estimated,
        partial,
    };
    assert_eq!(money(&value(false, false)).as_deref(), Some("$1.25"));
    assert_eq!(money(&value(true, false)).as_deref(), Some("≈$1.25"));
    assert_eq!(money(&value(true, true)).as_deref(), Some("≈$1.25+"));
}

#[test]
fn spending_uses_valid_configured_amount_or_tokens_never_unknown_dollars() {
    let mut status = ContextStatus {
        cumulative_tokens: Some(1050),
        ..Default::default()
    };
    assert_eq!(super::spending(&status), "1k tokens");
    status.cumulative_cost = Some(Money {
        currency: "USD".into(),
        amount: 1.25,
        estimated: true,
        partial: false,
    });
    assert_eq!(super::spending(&status), "≈$1.25");
    status.cumulative_cost.as_mut().unwrap().amount = f64::NAN;
    assert_eq!(super::spending(&status), "1k tokens");
    status.cumulative_tokens = None;
    assert_eq!(super::spending(&status), "? tokens");
}

#[test]
fn thresholds_use_highest_reported_pressure_and_require_known_capacity_for_tokens() {
    for (p, expected) in [
        (49.9, Pressure::Normal),
        (50., Pressure::Warning),
        (69.9, Pressure::Warning),
        (70., Pressure::High),
        (89.9, Pressure::High),
        (90., Pressure::Critical),
    ] {
        assert_eq!(
            pressure(&ContextStatus {
                percent: Some(p),
                ..Default::default()
            }),
            expected
        );
    }
    for (tokens, expected) in [
        (149_999, Pressure::Normal),
        (150_000, Pressure::Warning),
        (270_000, Pressure::High),
        (500_000, Pressure::Critical),
    ] {
        let mut status = ContextStatus {
            used_tokens: Some(tokens),
            context_window: Some(1_000_000),
            percent: Some(1.),
            ..Default::default()
        };
        assert_eq!(pressure(&status), expected);
        status.context_window = None;
        assert_eq!(pressure(&status), Pressure::Normal);
    }
    assert_eq!(
        pressure(&ContextStatus {
            percent: Some(68.9),
            used_tokens: Some(757900),
            context_window: Some(1100000),
            ..Default::default()
        }),
        Pressure::Critical
    );
}
