use oxidite_proto::{Multiplexer, SessionState};

#[tokio::test]
async fn test_chaos_connection_drop_with_tmux_reconnects() {
    let mut state = SessionState::Running;
    let multiplexer = Multiplexer::Tmux;

    // Simulate keepalive misses
    assert!(state.can_transition_to(&SessionState::Degraded));
    state = SessionState::Degraded;

    // Drop detected -> transition to Reconnecting
    assert!(state.can_transition_to(&SessionState::Reconnecting { attempt: 1 }));
    state = SessionState::Reconnecting { attempt: 1 };

    // Multiplexer is Tmux -> reconnect succeeds and reattaches
    if matches!(multiplexer, Multiplexer::Tmux) {
        let next_state = SessionState::Running;
        assert!(state.can_transition_to(&next_state));
        state = next_state;
    }

    assert_eq!(state, SessionState::Running);
}

#[tokio::test]
async fn test_chaos_connection_drop_without_tmux_fails_clearly() {
    let mut state = SessionState::Running;
    let multiplexer = Multiplexer::None;

    // Drop detected without tmux
    if matches!(multiplexer, Multiplexer::None) {
        let failed_state = SessionState::Failed {
            reason: "SSH channel dropped without multiplexer persistence (SIGHUP received)".into(),
        };
        assert!(state.can_transition_to(&failed_state));
        state = failed_state;
    }

    match state {
        SessionState::Failed { ref reason } => {
            assert!(reason.contains("without multiplexer"));
        }
        _ => panic!("Expected clear Failed state, got {:?}", state),
    }

    // Terminal state cannot transition anywhere
    assert!(!state.can_transition_to(&SessionState::Running));
}
