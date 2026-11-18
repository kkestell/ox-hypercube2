# Mutation testing of `oxd`'s tests

Which of `oxd`'s 250 tests catch mutants of `oxd`'s code, which catch
mutants no other test catches, and which code no test protects.

## What was measured

- **Commit:** `a33cf04` ("Clients state intent; oxd sequences session loads
  and config"), measured from a detached worktree of `HEAD`.
- **Tool:** `cargo-mutants` 27.1.0, run as `cargo mutants -p oxd -j 4
  --timeout 120 --build-timeout 300 --cargo-test-arg=--no-fail-fast
  --copy-target=true --no-shuffle`.
- **Mutated:** every file in `crates/oxd/src` except
  `src/bin/oxd-fake-agent.rs`. That file is the scripted agent the tests
  drive, not daemon code.
- **Tests run against each mutant:** all of `oxd`'s tests: 98 unit tests in
  `src/main.rs` and its modules, and the integration tests in `agents.rs`
  (12), `daemon.rs` (83), `import.rs` (6) and `worktrees.rs` (51).
  `--no-fail-fast` ran every test binary for every mutant that did not time
  out.
- **`ox-mcp`:** built once at the same commit into `target/debug`, and copied
  into each mutant's scratch build with the rest of `target/`. `ox-mcp` does
  not depend on `oxd`, so no mutant changes it. The unmutated baseline passed
  inside the scratch copy (17s build, 15s test). Before the run, four
  unmutated test runs at once all passed (60s each), so the tests hold up
  under `-j 4` load.
- **Order:** `state.rs`, `store.rs` and `session.rs` ran first (399 mutants),
  then the remaining files (378). All 777 mutants ran inside the 4-hour budget.

## Counts

| Outcome | Mutants |
| --- | ---: |
| Caught (at least one test failed) | 462 |
| Missed (every test passed) | 43 |
| Timed out (test run passed 120s) | 106 |
| Unviable (did not compile) | 166 |
| **Total** | **777** |

Of the 505 mutants that finished testing, 91.5% were caught. Counting the
timeouts as uncaught, the share is 75.6% (462 of 611 viable mutants).

**Wall time:** 2h01m, from 17:46:58 to 19:48:16 on 2026-11-17 (57m for the
first three files, 64m for the rest).

| Tests | Count | Share |
| --- | ---: | ---: |
| All tests | 250 | 100% |
| Caught no mutant (item 2) | 1 | 0.4% |
| Every catch also made by another test (item 3) | 210 | 84% |
| Caught at least one mutant alone | 39 | 16% |
| Greedy covering set | 64 | 26% |
| Deletion pool (outside the covering set) | 186 | 74% |

68 caught mutants were caught by exactly one test.

Timeouts are not counted as catches anywhere in this report. In all 106, at
least one test had already failed, or was still running at 60 seconds, when
the run was stopped. Test binaries run in the order unit, `agents`, `daemon`,
`import`, `worktrees`. The binaries after the one that hung never ran, so the
per-test data for timeouts is partial. The hanging binary was `worktrees` in
71 timeouts, `daemon` in 25, unit in 4, `agents` in 3 and `import` in 3.

## Outcomes by file

| File | Mutants | Caught | Missed | Timeout | Unviable |
| --- | ---: | ---: | ---: | ---: | ---: |
| `src/state.rs` | 224 | 158 | 9 | 17 | 40 |
| `src/daemon.rs` | 129 | 63 | 11 | 18 | 37 |
| `src/session.rs` | 124 | 62 | 4 | 20 | 38 |
| `src/worktree.rs` | 101 | 50 | 2 | 31 | 18 |
| `src/store.rs` | 51 | 39 | 0 | 0 | 12 |
| `src/conn.rs` | 34 | 23 | 3 | 6 | 2 |
| `src/transcript.rs` | 33 | 28 | 1 | 0 | 4 |
| `src/branches.rs` | 29 | 10 | 2 | 12 | 5 |
| `src/main.rs` | 17 | 12 | 2 | 1 | 2 |
| `src/directories.rs` | 17 | 8 | 6 | 0 | 3 |
| `src/import.rs` | 10 | 5 | 1 | 1 | 3 |
| `src/files.rs` | 7 | 4 | 2 | 0 | 1 |
| `src/mcp.rs` | 1 | 0 | 0 | 0 | 1 |
| **Total** | **777** | **462** | **43** | **106** | **166** |

## 1. Catches per test

*Caught* counts the mutants on which the test failed. *Alone* counts those on which it was the only failing test. *Timeouts* counts the timed-out mutants on which it failed or was still running at 60 seconds; these are not catches. *Cover* marks the tests in the greedy set cover below.

### `unit` (98 tests)

| Test | Caught | Alone | Timeouts | Cover |
| --- | ---: | ---: | ---: | :---: |
| `state::tests::a_steered_prompt_pending_at_a_turn_end_runs_before_the_queue` | 24 | 2 | 8 | ✓ |
| `state::tests::an_elicitation_awaits_input_unless_a_permission_request_is_open` | 22 | 0 | 4 | ✓ |
| `store::tests::events_round_trip_in_order_across_batches` | 22 | 0 | 0 |  |
| `state::tests::turns_that_end_other_than_normally_hold_the_queue_until_one_is_sent_now` | 20 | 0 | 8 |  |
| `store::tests::a_deleted_session_takes_its_worktree_and_events` | 20 | 0 | 0 | ✓ |
| `transcript::tests::events_fold_into_one_entry_each_keyed_by_their_first_event` | 19 | 4 | 0 | ✓ |
| `store::tests::an_imported_session_round_trips_as_archived_with_its_events` | 19 | 0 | 0 |  |
| `store::tests::unseen_is_saved_replaced_and_deleted_with_the_session` | 18 | 0 | 0 |  |
| `state::tests::steered_prompts_the_agent_does_not_take_are_dropped` | 17 | 1 | 7 | ✓ |
| `state::tests::a_rejected_prompt_holds_the_queue` | 17 | 0 | 6 | ✓ |
| `state::tests::transitions_record_the_changes_to_the_saved_session` | 17 | 0 | 5 |  |
| `store::tests::removing_a_workspace_deletes_its_sessions_without_remembering_them` | 17 | 0 | 0 |  |
| `store::tests::sessions_round_trip_in_the_order_they_were_created` | 17 | 0 | 0 |  |
| `state::tests::a_withdrawal_ends_only_the_elicitation_whose_receiver_closed` | 16 | 0 | 4 |  |
| `state::tests::cancelling_holds_the_queue_until_a_prompt_is_sent` | 16 | 0 | 8 |  |
| `store::tests::every_schema_fixture_opens_and_reads_back` | 16 | 0 | 0 |  |
| `transcript::tests::a_page_stops_at_the_budget_and_names_the_earlier_entries` | 16 | 0 | 0 |  |
| `store::tests::a_deleted_session_leaves_its_agent_session_id_remembered` | 15 | 0 | 0 | ✓ |
| `store::tests::a_forgotten_worktree_leaves_its_session_without_one` | 15 | 0 | 0 |  |
| `store::tests::a_saved_queue_comes_back_with_its_session_and_is_deleted_with_it` | 15 | 0 | 0 |  |
| `store::tests::a_session_round_trips_with_its_worktree` | 15 | 0 | 0 |  |
| `store::tests::archiving_is_saved_and_deleted_with_the_session` | 15 | 0 | 0 |  |
| `transcript::tests::an_entry_over_the_budget_is_cut` | 15 | 0 | 0 |  |
| `state::tests::a_session_is_quiet_when_the_agent_reports_no_background_work` | 14 | 0 | 6 |  |
| `store::tests::saved_title_agent_session_id_and_config_read_back` | 14 | 0 | 0 |  |
| `state::tests::a_moving_session_refuses_turns_and_loads_and_holds_its_queue` | 13 | 0 | 8 |  |
| `state::tests::a_restored_session_is_exited_and_loads_its_replay_as_history` | 13 | 0 | 6 |  |
| `state::tests::state_follows_the_turn_and_its_permission_requests` | 13 | 0 | 6 |  |
| `state::tests::a_settled_config_sets_only_the_settings_the_agent_accepted` | 12 | 2 | 0 | ✓ |
| `state::tests::a_first_prompt_is_recorded_while_starting_and_runs_once_started` | 12 | 1 | 6 | ✓ |
| `state::tests::a_peer_message_starts_a_turn_or_steers_with_its_sender_and_a_header_for_the_agent` | 12 | 0 | 6 | ✓ |
| `state::tests::a_prompt_during_a_turn_is_steered_and_taken_when_the_agent_accepts_it` | 12 | 0 | 6 |  |
| `state::tests::a_rejected_prompt_ends_the_turn_with_its_error` | 12 | 0 | 6 |  |
| `state::tests::queued_prompts_start_one_turn_each_in_order_after_normal_ends` | 12 | 0 | 8 |  |
| `store::tests::a_deleted_session_is_gone_and_its_number_is_not_reused` | 12 | 0 | 0 |  |
| `store::tests::a_failed_change_leaves_the_others_unwritten` | 12 | 0 | 0 |  |
| `store::tests::a_schema_1_database_gives_each_session_the_config_it_last_reported` | 12 | 0 | 0 | ✓ |
| `store::tests::a_session_whose_worktree_row_fails_is_not_saved` | 12 | 0 | 0 |  |
| `state::tests::nothing_follows_session_closed` | 11 | 1 | 6 | ✓ |
| `state::tests::a_late_rejection_leaves_the_next_prompt_running` | 11 | 0 | 6 | ✓ |
| `state::tests::a_rejected_prompt_after_the_agent_reported_work_waits_for_idle` | 11 | 0 | 6 |  |
| `state::tests::a_turn_ends_once` | 11 | 0 | 6 |  |
| `state::tests::the_resume_snapshot_gives_the_session_its_background_work` | 11 | 0 | 1 |  |
| `state::tests::the_resume_snapshot_sets_the_state_without_recording_a_turn` | 11 | 0 | 2 |  |
| `store::tests::workspaces_round_trip_in_the_order_they_were_added` | 11 | 0 | 0 |  |
| `state::tests::fill_closes_the_turn_and_the_permission_request_a_stop_left_open` | 10 | 2 | 2 | ✓ |
| `state::tests::queued_prompts_are_edited_in_place_and_removed_by_id` | 10 | 1 | 4 | ✓ |
| `state::tests::a_peer_message_after_a_cancelled_turn_leaves_the_queue_held` | 10 | 0 | 8 |  |
| `state::tests::fill_cancels_an_open_elicitation_and_ids_continue_past_it` | 10 | 0 | 1 | ✓ |
| `state::tests::permission_request_ids_continue_after_the_history` | 10 | 0 | 1 |  |
| `store::tests::a_workspace_path_is_saved_once` | 10 | 0 | 0 |  |
| `store::tests::events_of_a_deleted_session_are_dropped` | 10 | 0 | 0 |  |
| `state::tests::a_prompt_queued_on_an_idle_session_starts_at_once` | 9 | 0 | 5 |  |
| `state::tests::an_answered_prompt_that_was_rejected_after_its_turn_ended_changes_nothing` | 9 | 0 | 6 |  |
| `state::tests::cancelling_the_turn_leaves_an_elicitation_open` | 9 | 0 | 4 |  |
| `state::tests::the_agent_starts_and_ends_turns_of_its_own` | 9 | 0 | 2 |  |
| `state::tests::a_repeated_idle_snapshot_after_a_prompt_leaves_the_turn_running` | 8 | 0 | 6 |  |
| `store::tests::a_database_from_before_versions_opens_at_the_latest_version_with_its_rows` | 8 | 0 | 0 |  |
| `state::tests::a_load_that_fails_leaves_the_session_loadable` | 7 | 0 | 1 |  |
| `state::tests::exit_and_close_end_the_turn_and_its_permission_requests` | 7 | 0 | 6 |  |
| `state::tests::exit_park_and_close_cancel_an_elicitation` | 7 | 0 | 6 |  |
| `transcript::tests::each_kind_of_turn_end_has_its_line` | 7 | 0 | 0 |  |
| `state::tests::a_refused_send_leaves_the_prompt_queued` | 6 | 1 | 4 | ✓ |
| `state::tests::a_first_prompt_ends_when_the_agent_exits_while_starting` | 6 | 0 | 1 |  |
| `state::tests::a_load_that_fails_partway_through_its_replay_records_none_of_it` | 6 | 0 | 1 |  |
| `state::tests::a_move_begins_only_on_an_idle_or_exited_session` | 6 | 0 | 6 |  |
| `state::tests::session_info_updates_set_clear_and_keep_the_title` | 6 | 0 | 1 |  |
| `store::tests::a_new_database_has_every_table_at_the_latest_version` | 6 | 0 | 0 |  |
| `daemon::tests::only_clients_and_live_sessions_keep_the_daemon_running` | 5 | 2 | 0 | ✓ |
| `session::tests::parkability_holds_until_a_lapse` | 5 | 1 | 4 | ✓ |
| `session::tests::a_lapse_while_the_park_waits_for_the_lifecycle_restarts_the_delay` | 5 | 0 | 4 |  |
| `state::tests::a_session_with_a_config_keeps_it_when_the_agent_reports_another` | 5 | 0 | 1 |  |
| `state::tests::an_appended_event_carries_the_time_it_was_recorded` | 5 | 0 | 1 |  |
| `state::tests::exit_ends_a_turn_the_agent_started` | 5 | 0 | 1 |  |
| `state::tests::states_other_than_idle_are_work` | 5 | 0 | 2 |  |
| `import::tests::user_messages_start_turns_that_end_before_the_next` | 4 | 0 | 1 |  |
| `state::tests::a_failed_start_forgets_the_worktree` | 4 | 0 | 1 |  |
| `state::tests::a_restored_session_that_never_started_holds_its_first_prompt_and_cannot_load` | 4 | 0 | 0 |  |
| `state::tests::a_session_with_a_history_loads_without_duplicating_it` | 4 | 0 | 2 |  |
| `state::tests::fill_leaves_a_finished_history_as_it_is` | 4 | 0 | 0 |  |
| `state::tests::forms_a_client_cannot_render_are_refused` | 4 | 0 | 0 |  |
| `state::tests::reported_config_reads_model_effort_and_mode_by_category` | 4 | 0 | 0 |  |
| `state::tests::session_closed_is_not_logged` | 4 | 0 | 1 |  |
| `state::tests::unseen_reads_how_turns_and_agents_ended` | 4 | 0 | 0 |  |
| `state::tests::commands_replace_the_catalog_including_on_resume_without_replay` | 3 | 1 | 1 | ✓ |
| `session::tests::a_lapse_that_reverts_before_the_park_task_looks_restarts_the_delay` | 3 | 0 | 4 |  |
| `state::tests::a_restored_session_holds_no_events_until_it_is_filled` | 3 | 0 | 0 |  |
| `state::tests::a_restored_session_whose_history_has_a_prompt_loads_without_replay` | 3 | 0 | 0 |  |
| `state::tests::parking_exits_a_quiet_session_and_records_it` | 3 | 0 | 1 |  |
| `store::tests::a_database_of_a_newer_schema_is_refused` | 3 | 0 | 0 |  |
| `import::tests::updates_without_a_prompt_end_no_turn` | 2 | 0 | 1 |  |
| `session::tests::a_parkable_session_is_parked_once_the_delay_passes` | 2 | 0 | 4 |  |
| `session::tests::a_session_is_parked_only_while_it_stays_parkable` | 2 | 0 | 4 |  |
| `state::tests::an_untitled_sender_is_named_by_its_id_alone` | 2 | 0 | 0 |  |
| `daemon::tests::a_change_restarts_the_delay_and_busy_time_does_not_count` | 1 | 0 | 0 |  |
| `daemon::tests::idling_for_the_delay_resolves` | 1 | 0 | 0 |  |
| `state::tests::only_an_exited_session_with_an_agent_session_loads` | 1 | 0 | 1 |  |
| `transcript::tests::an_empty_history_has_no_entries` | 0 | 0 | 0 |  |

### `agents` (12 tests)

| Test | Caught | Alone | Timeouts | Cover |
| --- | ---: | ---: | ---: | :---: |
| `a_message_wakes_a_parked_session_with_the_senders_environment` | 113 | 5 | 32 | ✓ |
| `a_message_loads_a_session_restored_after_a_restart` | 104 | 0 | 32 |  |
| `an_approved_request_starts_the_session_it_describes` | 93 | 0 | 45 | ✓ |
| `read_transcript_pages_through_any_sessions_transcript` | 88 | 1 | 29 | ✓ |
| `a_message_to_an_idle_session_starts_a_turn_with_its_sender` | 74 | 0 | 28 |  |
| `list_sessions_lists_the_sessions_one_can_message` | 73 | 0 | 35 |  |
| `a_message_to_a_busy_session_steers_its_turn` | 72 | 0 | 28 |  |
| `messages_to_sessions_that_cannot_take_them_are_refused` | 72 | 0 | 37 |  |
| `a_declined_request_creates_nothing` | 71 | 0 | 30 |  |
| `a_request_that_cannot_be_started_is_refused_without_asking` | 69 | 1 | 39 | ✓ |
| `a_request_in_the_workspaces_checkout_names_its_directory` | 63 | 0 | 26 |  |
| `list_harnesses_lists_the_config` | 59 | 0 | 26 |  |

### `daemon` (83 tests)

| Test | Caught | Alone | Timeouts | Cover |
| --- | ---: | ---: | ---: | :---: |
| `focusing_one_exited_session_after_another_loads_both` | 114 | 0 | 27 |  |
| `a_held_queue_survives_a_restart` | 106 | 0 | 28 |  |
| `loading_a_restored_session_keeps_its_stored_history` | 103 | 0 | 29 |  |
| `a_session_resumed_while_its_agent_is_working_runs_until_the_agent_is_idle` | 98 | 0 | 29 |  |
| `a_parked_session_loads_again` | 93 | 0 | 29 |  |
| `a_history_that_a_crash_left_open_ends_its_turn` | 91 | 0 | 20 |  |
| `loading_a_restored_session_recovers_missing_history_from_the_agent` | 89 | 0 | 29 |  |
| `a_session_with_background_work_is_not_parked` | 88 | 0 | 27 |  |
| `closing_the_focusing_connection_ends_its_focus` | 87 | 0 | 20 | ✓ |
| `unseen_outcomes_survive_a_restart` | 84 | 0 | 28 |  |
| `a_viewed_session_is_not_parked` | 83 | 0 | 28 |  |
| `workspaces_and_sessions_survive_a_restart` | 83 | 0 | 27 | ✓ |
| `a_stopped_daemon_stores_the_end_of_the_turn_it_interrupted` | 82 | 0 | 26 | ✓ |
| `a_loaded_session_sends_its_recorded_config_again` | 80 | 0 | 29 |  |
| `an_archived_session_that_never_started_keeps_its_history_across_a_restart` | 80 | 0 | 19 |  |
| `a_loaded_agent_gets_new_permission_request_ids` | 78 | 2 | 29 | ✓ |
| `a_turn_that_ends_out_of_focus_is_unseen_until_the_session_is_focused` | 78 | 0 | 19 |  |
| `stop_holds_the_queue_and_drops_a_prompt_the_agent_has_not_taken` | 77 | 1 | 25 | ✓ |
| `a_session_whose_agent_reports_no_background_work_is_not_parked` | 77 | 0 | 19 | ✓ |
| `a_cancelled_turn_is_not_unseen_and_an_error_outlasts_a_finish` | 75 | 1 | 27 | ✓ |
| `a_daemon_whose_sessions_are_parked_exits` | 74 | 0 | 14 | ✓ |
| `a_deleted_session_loses_its_history` | 74 | 0 | 17 |  |
| `a_turn_after_the_resume_answer_is_live` | 74 | 0 | 27 |  |
| `loading_a_session_whose_agent_exited_keeps_its_history` | 74 | 0 | 29 |  |
| `exited_sessions_do_not_keep_the_daemon_alive` | 73 | 0 | 24 |  |
| `queued_prompts_are_edited_and_removed_until_they_are_sent` | 70 | 2 | 24 | ✓ |
| `a_new_sessions_first_prompt_shows_before_its_agent_starts` | 70 | 0 | 18 |  |
| `cancel_withdraws_an_elicitation` | 70 | 0 | 25 | ✓ |
| `set_config_sets_only_its_settings_and_the_next_prompt_runs_with_them` | 70 | 0 | 25 |  |
| `queued_prompts_are_sent_when_turns_end_with_no_client_connected` | 69 | 0 | 25 |  |
| `watch_sends_only_sessions_whose_info_changed` | 69 | 0 | 28 |  |
| `a_daemon_with_a_running_turn_refuses_to_stop` | 67 | 1 | 23 | ✓ |
| `a_setting_the_agent_fails_rejects_the_first_prompt` | 67 | 1 | 25 | ✓ |
| `a_workspace_is_removed_once_its_sessions_are_archived` | 67 | 0 | 40 |  |
| `elicitation_round_trip` | 67 | 0 | 23 |  |
| `session_info_updates_set_the_title` | 67 | 0 | 25 |  |
| `an_idle_unviewed_session_is_parked_and_its_agent_exits` | 66 | 0 | 14 |  |
| `agent_sets_title_through_ox_mcp_server` | 65 | 0 | 25 |  |
| `permission_across_disconnect` | 65 | 0 | 24 |  |
| `reattach_mid_turn` | 65 | 0 | 25 |  |
| `unsubscribe_ends_a_watch` | 65 | 0 | 23 |  |
| `prompt_content_reaches_the_agent` | 64 | 3 | 25 | ✓ |
| `a_new_session_shows_its_config_throughout_and_sets_it_before_its_first_prompt` | 64 | 0 | 17 |  |
| `a_refused_setting_is_named_and_the_accepted_ones_are_kept` | 64 | 0 | 15 |  |
| `concurrent_set_config_requests_keep_every_setting` | 63 | 0 | 15 |  |
| `permission_after_turn_returns_session_to_idle` | 63 | 0 | 25 |  |
| `unsubscribe_ends_a_subscription` | 63 | 0 | 25 |  |
| `subscriptions_to_one_session_are_independent` | 62 | 1 | 18 | ✓ |
| `a_late_rejection_does_not_end_the_next_prompt` | 62 | 0 | 23 |  |
| `cancel_with_pending_permission` | 62 | 0 | 23 |  |
| `focus_loads_only_an_exited_session_that_started` | 62 | 0 | 21 |  |
| `vanished_client` | 62 | 0 | 24 |  |
| `a_prompt_sent_during_a_turn_is_taken_into_it` | 61 | 0 | 25 |  |
| `a_turn_the_agent_starts_runs_until_the_agent_is_idle` | 61 | 0 | 25 | ✓ |
| `prompt_content_the_agent_does_not_accept_is_rejected` | 61 | 0 | 25 |  |
| `agent_gets_ox_instructions` | 60 | 1 | 27 | ✓ |
| `a_stopping_daemon_sends_the_rest_of_each_subscription` | 60 | 0 | 19 |  |
| `slow_subscriber_receives_every_event` | 60 | 0 | 17 |  |
| `delete_while_starting` | 59 | 1 | 15 | ✓ |
| `a_prompt_the_agent_rejects_ends_its_turn_with_the_error` | 59 | 0 | 23 |  |
| `a_turn_ends_with_the_stop_reason_the_agent_reports` | 59 | 0 | 25 |  |
| `lifetime` | 59 | 0 | 15 |  |
| `delete_ends_the_turn_and_its_permission_request` | 58 | 0 | 27 |  |
| `agent_exit` | 57 | 0 | 25 |  |
| `exhausted_descriptors_do_not_stop_the_daemon` | 55 | 1 | 15 | ✓ |
| `delete_ends_other_clients_subscriptions` | 54 | 0 | 18 |  |
| `a_first_prompt_ends_when_the_agent_does_not_start` | 53 | 0 | 13 |  |
| `a_session_runs_in_its_workspace` | 53 | 0 | 16 |  |
| `sigterm_stops_agents_and_removes_the_socket` | 53 | 0 | 14 |  |
| `delete_stops_the_agent` | 52 | 0 | 16 |  |
| `an_agent_without_session_capabilities_cannot_run_sessions` | 51 | 0 | 13 |  |
| `directory_watch_lists_a_directory_once_it_exists` | 38 | 1 | 2 | ✓ |
| `file_watch_follows_edits_replacement_and_recreation` | 37 | 6 | 2 | ✓ |
| `directory_watch_follows_changes` | 35 | 1 | 2 | ✓ |
| `a_first_prompt_with_content_beyond_text_and_links_is_refused` | 34 | 0 | 5 |  |
| `workspace_add_refuses_what_is_not_a_new_directory` | 33 | 0 | 8 |  |
| `focusing_an_unknown_session_is_refused` | 26 | 0 | 2 |  |
| `an_idle_daemon_stops_when_asked_before_hello` | 25 | 0 | 1 |  |
| `version_mismatch` | 24 | 2 | 2 | ✓ |
| `single_instance_and_on_demand_start` | 24 | 0 | 5 |  |
| `oxd_stop_returns_once_an_idle_daemon_has_stopped` | 22 | 0 | 1 | ✓ |
| `socket_is_private` | 15 | 0 | 1 |  |
| `oxd_stop_succeeds_with_no_daemon_running` | 5 | 1 | 0 | ✓ |

### `import` (6 tests)

| Test | Caught | Alone | Timeouts | Cover |
| --- | ---: | ---: | ---: | :---: |
| `an_unarchived_imported_session_resumes_without_replay` | 98 | 0 | 20 | ✓ |
| `importing_adds_each_session_archived_with_its_history` | 76 | 0 | 12 |  |
| `removing_a_workspace_of_archived_sessions_lets_them_be_imported_again` | 57 | 0 | 14 |  |
| `a_deleted_imported_session_is_not_imported_again` | 55 | 0 | 11 |  |
| `importing_again_adds_only_new_sessions` | 54 | 3 | 10 | ✓ |
| `a_transcript_that_fails_is_skipped` | 49 | 0 | 10 |  |

### `worktrees` (51 tests)

| Test | Caught | Alone | Timeouts | Cover |
| --- | ---: | ---: | ---: | :---: |
| `archiving_removes_the_worktree_and_keeps_the_branch_and_the_conversation` | 115 | 0 | 44 | ✓ |
| `a_delete_compares_the_repositorys_sessions_again` | 83 | 0 | 40 | ✓ |
| `uncommitted_changes_refuse_the_delete_until_it_discards_them` | 81 | 0 | 37 |  |
| `a_worktree_session_that_never_started_keeps_its_first_prompt_across_a_restart` | 79 | 0 | 20 |  |
| `a_locked_worktree_fails_the_delete_and_the_session_loads_again` | 78 | 0 | 39 |  |
| `deleting_an_archived_session_removes_it_and_applies_the_branch_rule` | 77 | 0 | 37 |  |
| `a_worktree_session_survives_a_restart_and_loads_only_while_its_directory_exists` | 75 | 0 | 32 |  |
| `a_queued_prompt_waits_for_a_branch_move_in_its_checkout` | 74 | 0 | 43 | ✓ |
| `a_turn_end_compares_again` | 74 | 0 | 33 |  |
| `a_worktree_that_cannot_be_added_is_undone` | 74 | 0 | 30 |  |
| `a_branch_merged_elsewhere_is_deleted_and_one_with_its_own_commits_is_kept` | 73 | 0 | 42 |  |
| `a_clean_delete_removes_the_worktree_and_the_branch` | 73 | 0 | 41 |  |
| `a_worktree_deleted_outside_ox_is_unregistered` | 72 | 2 | 40 | ✓ |
| `a_branch_is_kept_until_its_commits_are_pushed` | 72 | 0 | 42 |  |
| `a_session_whose_workspace_subdirectory_is_gone_is_deleted` | 72 | 0 | 42 |  |
| `a_session_imported_during_a_branch_move_refuses_loads_until_it_ends` | 71 | 1 | 52 | ✓ |
| `a_comparison_ref_survives_a_restart_and_is_compared_unasked` | 71 | 0 | 30 |  |
| `a_worktree_whose_repository_moved_is_removed_only_with_discard` | 69 | 1 | 34 | ✓ |
| `removing_a_workspace_waits_for_an_unarchive_under_way` | 68 | 0 | 40 |  |
| `a_branch_another_worktree_has_checked_out_is_kept` | 67 | 0 | 38 | ✓ |
| `a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn` | 67 | 0 | 42 |  |
| `a_session_runs_in_a_worktree_on_an_existing_branch` | 67 | 0 | 38 |  |
| `a_move_compares_the_repositorys_sessions_again` | 66 | 0 | 47 |  |
| `a_worktree_whose_registration_was_removed_is_removed_only_with_discard` | 66 | 0 | 33 | ✓ |
| `a_workspace_removed_during_a_creation` | 65 | 0 | 36 | ✓ |
| `archiving_a_running_session_ends_its_turn` | 65 | 0 | 24 |  |
| `archiving_refuses_unsaved_work_unless_it_is_discarded` | 64 | 0 | 39 |  |
| `unarchiving_again_finishes_when_the_worktree_is_already_back` | 64 | 0 | 37 |  |
| `submodule_commits_on_no_remote_branch_refuse_the_delete` | 62 | 2 | 35 | ✓ |
| `a_worktree_is_listed_before_its_session_and_removed_with_it` | 61 | 0 | 25 |  |
| `a_failed_worktree_leaves_a_branch_another_worktree_checked_out` | 60 | 0 | 30 | ✓ |
| `a_worktree_sessions_first_prompt_runs_in_the_worktree_once_added` | 60 | 0 | 28 |  |
| `a_comparison_is_refreshed_on_request` | 59 | 0 | 30 |  |
| `a_delete_during_startup_waits_for_the_worktree` | 59 | 0 | 39 |  |
| `commits_on_no_branch_refuse_the_delete` | 57 | 0 | 28 |  |
| `unarchiving_needs_a_branch_git_can_check_out` | 57 | 0 | 39 |  |
| `a_worktree_that_cannot_be_added_ends_the_first_prompts_turn` | 56 | 0 | 27 |  |
| `a_subdirectory_workspace_runs_in_the_same_subdirectory` | 54 | 1 | 29 | ✓ |
| `a_session_runs_in_a_new_worktree_on_a_new_branch` | 54 | 0 | 29 |  |
| `git_ignores_the_clients_repository_variables` | 54 | 0 | 25 |  |
| `a_linked_worktree_workspace_names_the_worktree_after_itself` | 53 | 0 | 27 |  |
| `a_new_session_publishes_its_comparison` | 49 | 0 | 30 |  |
| `workspaces_say_whether_they_are_in_a_git_working_tree` | 44 | 0 | 6 |  |
| `a_move_keeps_an_ignored_file_it_would_overwrite` | 40 | 4 | 36 | ✓ |
| `a_git_directory_git_rejects_is_refused_with_gits_reason` | 37 | 2 | 16 | ✓ |
| `a_move_keeps_uncommitted_and_untracked_work` | 37 | 0 | 43 |  |
| `moving_a_checked_out_branch_moves_its_checkout` | 35 | 0 | 34 |  |
| `a_branch_no_checkout_has_moves_only_from_where_it_is_expected` | 32 | 0 | 41 |  |
| `a_request_git_cannot_satisfy_is_refused_with_nothing_created` | 32 | 0 | 18 |  |
| `branches_are_refused_outside_a_working_tree` | 32 | 0 | 7 |  |
| `branches_name_the_branch_the_checkout_is_on` | 32 | 0 | 18 |  |

## 2. Tests that caught no mutant

| Test | Binary | Timeouts |
| --- | --- | ---: |
| `transcript::tests::an_empty_history_has_no_entries` | `unit` | 0 |

## 3. Tests whose every catch another test also made

210 tests. Each row gives the binary, the count, and the tests.

| Binary | Count | Tests |
| --- | ---: | --- |
| `unit` | 85 | `daemon::tests::a_change_restarts_the_delay_and_busy_time_does_not_count`, `daemon::tests::idling_for_the_delay_resolves`, `import::tests::updates_without_a_prompt_end_no_turn`, `import::tests::user_messages_start_turns_that_end_before_the_next`, `session::tests::a_lapse_that_reverts_before_the_park_task_looks_restarts_the_delay`, `session::tests::a_lapse_while_the_park_waits_for_the_lifecycle_restarts_the_delay`, `session::tests::a_parkable_session_is_parked_once_the_delay_passes`, `session::tests::a_session_is_parked_only_while_it_stays_parkable`, `state::tests::a_failed_start_forgets_the_worktree`, `state::tests::a_first_prompt_ends_when_the_agent_exits_while_starting`, `state::tests::a_late_rejection_leaves_the_next_prompt_running`, `state::tests::a_load_that_fails_leaves_the_session_loadable`, `state::tests::a_load_that_fails_partway_through_its_replay_records_none_of_it`, `state::tests::a_move_begins_only_on_an_idle_or_exited_session`, `state::tests::a_moving_session_refuses_turns_and_loads_and_holds_its_queue`, `state::tests::a_peer_message_after_a_cancelled_turn_leaves_the_queue_held`, `state::tests::a_peer_message_starts_a_turn_or_steers_with_its_sender_and_a_header_for_the_agent`, `state::tests::a_prompt_during_a_turn_is_steered_and_taken_when_the_agent_accepts_it`, `state::tests::a_prompt_queued_on_an_idle_session_starts_at_once`, `state::tests::a_rejected_prompt_after_the_agent_reported_work_waits_for_idle`, `state::tests::a_rejected_prompt_ends_the_turn_with_its_error`, `state::tests::a_rejected_prompt_holds_the_queue`, `state::tests::a_repeated_idle_snapshot_after_a_prompt_leaves_the_turn_running`, `state::tests::a_restored_session_holds_no_events_until_it_is_filled`, `state::tests::a_restored_session_is_exited_and_loads_its_replay_as_history`, `state::tests::a_restored_session_that_never_started_holds_its_first_prompt_and_cannot_load`, `state::tests::a_restored_session_whose_history_has_a_prompt_loads_without_replay`, `state::tests::a_session_is_quiet_when_the_agent_reports_no_background_work`, `state::tests::a_session_with_a_config_keeps_it_when_the_agent_reports_another`, `state::tests::a_session_with_a_history_loads_without_duplicating_it`, `state::tests::a_turn_ends_once`, `state::tests::a_withdrawal_ends_only_the_elicitation_whose_receiver_closed`, `state::tests::an_answered_prompt_that_was_rejected_after_its_turn_ended_changes_nothing`, `state::tests::an_appended_event_carries_the_time_it_was_recorded`, `state::tests::an_elicitation_awaits_input_unless_a_permission_request_is_open`, `state::tests::an_untitled_sender_is_named_by_its_id_alone`, `state::tests::cancelling_holds_the_queue_until_a_prompt_is_sent`, `state::tests::cancelling_the_turn_leaves_an_elicitation_open`, `state::tests::exit_and_close_end_the_turn_and_its_permission_requests`, `state::tests::exit_ends_a_turn_the_agent_started`, `state::tests::exit_park_and_close_cancel_an_elicitation`, `state::tests::fill_cancels_an_open_elicitation_and_ids_continue_past_it`, `state::tests::fill_leaves_a_finished_history_as_it_is`, `state::tests::forms_a_client_cannot_render_are_refused`, `state::tests::only_an_exited_session_with_an_agent_session_loads`, `state::tests::parking_exits_a_quiet_session_and_records_it`, `state::tests::permission_request_ids_continue_after_the_history`, `state::tests::queued_prompts_start_one_turn_each_in_order_after_normal_ends`, `state::tests::reported_config_reads_model_effort_and_mode_by_category`, `state::tests::session_closed_is_not_logged`, `state::tests::session_info_updates_set_clear_and_keep_the_title`, `state::tests::state_follows_the_turn_and_its_permission_requests`, `state::tests::states_other_than_idle_are_work`, `state::tests::the_agent_starts_and_ends_turns_of_its_own`, `state::tests::the_resume_snapshot_gives_the_session_its_background_work`, `state::tests::the_resume_snapshot_sets_the_state_without_recording_a_turn`, `state::tests::transitions_record_the_changes_to_the_saved_session`, `state::tests::turns_that_end_other_than_normally_hold_the_queue_until_one_is_sent_now`, `state::tests::unseen_reads_how_turns_and_agents_ended`, `store::tests::a_database_from_before_versions_opens_at_the_latest_version_with_its_rows`, `store::tests::a_database_of_a_newer_schema_is_refused`, `store::tests::a_deleted_session_is_gone_and_its_number_is_not_reused`, `store::tests::a_deleted_session_leaves_its_agent_session_id_remembered`, `store::tests::a_deleted_session_takes_its_worktree_and_events`, `store::tests::a_failed_change_leaves_the_others_unwritten`, `store::tests::a_forgotten_worktree_leaves_its_session_without_one`, `store::tests::a_new_database_has_every_table_at_the_latest_version`, `store::tests::a_saved_queue_comes_back_with_its_session_and_is_deleted_with_it`, `store::tests::a_schema_1_database_gives_each_session_the_config_it_last_reported`, `store::tests::a_session_round_trips_with_its_worktree`, `store::tests::a_session_whose_worktree_row_fails_is_not_saved`, `store::tests::a_workspace_path_is_saved_once`, `store::tests::an_imported_session_round_trips_as_archived_with_its_events`, `store::tests::archiving_is_saved_and_deleted_with_the_session`, `store::tests::events_of_a_deleted_session_are_dropped`, `store::tests::events_round_trip_in_order_across_batches`, `store::tests::every_schema_fixture_opens_and_reads_back`, `store::tests::removing_a_workspace_deletes_its_sessions_without_remembering_them`, `store::tests::saved_title_agent_session_id_and_config_read_back`, `store::tests::sessions_round_trip_in_the_order_they_were_created`, `store::tests::unseen_is_saved_replaced_and_deleted_with_the_session`, `store::tests::workspaces_round_trip_in_the_order_they_were_added`, `transcript::tests::a_page_stops_at_the_budget_and_names_the_earlier_entries`, `transcript::tests::an_entry_over_the_budget_is_cut`, `transcript::tests::each_kind_of_turn_end_has_its_line` |
| `agents` | 9 | `a_declined_request_creates_nothing`, `a_message_loads_a_session_restored_after_a_restart`, `a_message_to_a_busy_session_steers_its_turn`, `a_message_to_an_idle_session_starts_a_turn_with_its_sender`, `a_request_in_the_workspaces_checkout_names_its_directory`, `an_approved_request_starts_the_session_it_describes`, `list_harnesses_lists_the_config`, `list_sessions_lists_the_sessions_one_can_message`, `messages_to_sessions_that_cannot_take_them_are_refused` |
| `daemon` | 67 | `a_daemon_whose_sessions_are_parked_exits`, `a_deleted_session_loses_its_history`, `a_first_prompt_ends_when_the_agent_does_not_start`, `a_first_prompt_with_content_beyond_text_and_links_is_refused`, `a_held_queue_survives_a_restart`, `a_history_that_a_crash_left_open_ends_its_turn`, `a_late_rejection_does_not_end_the_next_prompt`, `a_loaded_session_sends_its_recorded_config_again`, `a_new_session_shows_its_config_throughout_and_sets_it_before_its_first_prompt`, `a_new_sessions_first_prompt_shows_before_its_agent_starts`, `a_parked_session_loads_again`, `a_prompt_sent_during_a_turn_is_taken_into_it`, `a_prompt_the_agent_rejects_ends_its_turn_with_the_error`, `a_refused_setting_is_named_and_the_accepted_ones_are_kept`, `a_session_resumed_while_its_agent_is_working_runs_until_the_agent_is_idle`, `a_session_runs_in_its_workspace`, `a_session_whose_agent_reports_no_background_work_is_not_parked`, `a_session_with_background_work_is_not_parked`, `a_stopped_daemon_stores_the_end_of_the_turn_it_interrupted`, `a_stopping_daemon_sends_the_rest_of_each_subscription`, `a_turn_after_the_resume_answer_is_live`, `a_turn_ends_with_the_stop_reason_the_agent_reports`, `a_turn_that_ends_out_of_focus_is_unseen_until_the_session_is_focused`, `a_turn_the_agent_starts_runs_until_the_agent_is_idle`, `a_viewed_session_is_not_parked`, `a_workspace_is_removed_once_its_sessions_are_archived`, `agent_exit`, `agent_sets_title_through_ox_mcp_server`, `an_agent_without_session_capabilities_cannot_run_sessions`, `an_archived_session_that_never_started_keeps_its_history_across_a_restart`, `an_idle_daemon_stops_when_asked_before_hello`, `an_idle_unviewed_session_is_parked_and_its_agent_exits`, `cancel_with_pending_permission`, `cancel_withdraws_an_elicitation`, `closing_the_focusing_connection_ends_its_focus`, `concurrent_set_config_requests_keep_every_setting`, `delete_ends_other_clients_subscriptions`, `delete_ends_the_turn_and_its_permission_request`, `delete_stops_the_agent`, `elicitation_round_trip`, `exited_sessions_do_not_keep_the_daemon_alive`, `focus_loads_only_an_exited_session_that_started`, `focusing_an_unknown_session_is_refused`, `focusing_one_exited_session_after_another_loads_both`, `lifetime`, `loading_a_restored_session_keeps_its_stored_history`, `loading_a_restored_session_recovers_missing_history_from_the_agent`, `loading_a_session_whose_agent_exited_keeps_its_history`, `oxd_stop_returns_once_an_idle_daemon_has_stopped`, `permission_across_disconnect`, `permission_after_turn_returns_session_to_idle`, `prompt_content_the_agent_does_not_accept_is_rejected`, `queued_prompts_are_sent_when_turns_end_with_no_client_connected`, `reattach_mid_turn`, `session_info_updates_set_the_title`, `set_config_sets_only_its_settings_and_the_next_prompt_runs_with_them`, `sigterm_stops_agents_and_removes_the_socket`, `single_instance_and_on_demand_start`, `slow_subscriber_receives_every_event`, `socket_is_private`, `unseen_outcomes_survive_a_restart`, `unsubscribe_ends_a_subscription`, `unsubscribe_ends_a_watch`, `vanished_client`, `watch_sends_only_sessions_whose_info_changed`, `workspace_add_refuses_what_is_not_a_new_directory`, `workspaces_and_sessions_survive_a_restart` |
| `import` | 5 | `a_deleted_imported_session_is_not_imported_again`, `a_transcript_that_fails_is_skipped`, `an_unarchived_imported_session_resumes_without_replay`, `importing_adds_each_session_archived_with_its_history`, `removing_a_workspace_of_archived_sessions_lets_them_be_imported_again` |
| `worktrees` | 44 | `a_branch_another_worktree_has_checked_out_is_kept`, `a_branch_is_kept_until_its_commits_are_pushed`, `a_branch_merged_elsewhere_is_deleted_and_one_with_its_own_commits_is_kept`, `a_branch_no_checkout_has_moves_only_from_where_it_is_expected`, `a_clean_delete_removes_the_worktree_and_the_branch`, `a_comparison_is_refreshed_on_request`, `a_comparison_ref_survives_a_restart_and_is_compared_unasked`, `a_delete_compares_the_repositorys_sessions_again`, `a_delete_during_startup_waits_for_the_worktree`, `a_failed_worktree_leaves_a_branch_another_worktree_checked_out`, `a_linked_worktree_workspace_names_the_worktree_after_itself`, `a_locked_worktree_fails_the_delete_and_the_session_loads_again`, `a_move_compares_the_repositorys_sessions_again`, `a_move_keeps_uncommitted_and_untracked_work`, `a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn`, `a_new_session_publishes_its_comparison`, `a_queued_prompt_waits_for_a_branch_move_in_its_checkout`, `a_request_git_cannot_satisfy_is_refused_with_nothing_created`, `a_session_runs_in_a_new_worktree_on_a_new_branch`, `a_session_runs_in_a_worktree_on_an_existing_branch`, `a_session_whose_workspace_subdirectory_is_gone_is_deleted`, `a_turn_end_compares_again`, `a_workspace_removed_during_a_creation`, `a_worktree_is_listed_before_its_session_and_removed_with_it`, `a_worktree_session_survives_a_restart_and_loads_only_while_its_directory_exists`, `a_worktree_session_that_never_started_keeps_its_first_prompt_across_a_restart`, `a_worktree_sessions_first_prompt_runs_in_the_worktree_once_added`, `a_worktree_that_cannot_be_added_ends_the_first_prompts_turn`, `a_worktree_that_cannot_be_added_is_undone`, `a_worktree_whose_registration_was_removed_is_removed_only_with_discard`, `archiving_a_running_session_ends_its_turn`, `archiving_refuses_unsaved_work_unless_it_is_discarded`, `archiving_removes_the_worktree_and_keeps_the_branch_and_the_conversation`, `branches_are_refused_outside_a_working_tree`, `branches_name_the_branch_the_checkout_is_on`, `commits_on_no_branch_refuse_the_delete`, `deleting_an_archived_session_removes_it_and_applies_the_branch_rule`, `git_ignores_the_clients_repository_variables`, `moving_a_checked_out_branch_moves_its_checkout`, `removing_a_workspace_waits_for_an_unarchive_under_way`, `unarchiving_again_finishes_when_the_worktree_is_already_back`, `unarchiving_needs_a_branch_git_can_check_out`, `uncommitted_changes_refuse_the_delete_until_it_discards_them`, `workspaces_say_whether_they_are_in_a_git_working_tree` |

## 4. Mutants no test caught

| File | Function | Mutants |
| --- | --- | --- |
| `src/branches.rs` | `collides` | `L285: replace == with != in collides`<br>`L285: replace && with \|\| in collides` |
| `src/conn.rs` | `forward` | `L308: replace \|\| with && in forward` |
| `src/conn.rs` | `serve` | `L199: delete match arm Method::SessionSetConfig{session_id, config} in serve`<br>`L107: replace match guard !greeted with false in serve` |
| `src/daemon.rs` | `Daemon::flush` | `L248: replace Daemon::flush with ()` |
| `src/daemon.rs` | `Daemon::is_idle` | `L223: replace Daemon::is_idle -> bool with true` |
| `src/daemon.rs` | `Daemon::listing_changes` | `L511: delete ! in Daemon::listing_changes`<br>`L511: replace == with != in Daemon::listing_changes` |
| `src/daemon.rs` | `Daemon::remove_workspace` | `L597: replace < with > in Daemon::remove_workspace`<br>`L613: replace \|\| with && in Daemon::remove_workspace` |
| `src/daemon.rs` | `Daemon::request_session` | `L917: replace == with != in Daemon::request_session` |
| `src/daemon.rs` | `Registry::refuse_moving` | `L109: replace Registry::refuse_moving -> Result<(), ErrorBody> with Ok(())` |
| `src/daemon.rs` | `now` | `L1457: replace now -> u64 with 0`<br>`L1457: replace now -> u64 with 1` |
| `src/daemon.rs` | `resolved` | `L1465: replace resolved -> PathBuf with Default::default()` |
| `src/directories.rs` | `<impl Drop for Changes>::drop` | `L181: replace <impl Drop for Changes>::drop with ()`<br>`L186: replace -= with += in <impl Drop for Changes>::drop`<br>`L186: replace -= with /= in <impl Drop for Changes>::drop` |
| `src/directories.rs` | `Changes::changed` | `L175: replace Changes::changed with ()` |
| `src/directories.rs` | `Changes::is_target` | `L164: replace Changes::is_target -> bool with true` |
| `src/directories.rs` | `Changes::mark_seen` | `L169: replace Changes::mark_seen with ()` |
| `src/files.rs` | `stream` | `L48: delete ! in stream`<br>`L48: replace \|\| with && in stream` |
| `src/import.rs` | `events` | `L87: replace match guard message .content .value() .is_some_and(\|content\| !content.is_empty()) with true in events` |
| `src/main.rs` | `run` | `L171: replace match guard error.kind() != io::ErrorKind::NotFound with false in run` |
| `src/main.rs` | `stop` | `L250: replace match guard matches!( error.kind(), io::ErrorKind::NotFound \| io::ErrorKind::ConnectionRefused ) with true in stop` |
| `src/session.rs` | `Session::events_after` | `L574: replace > with >= in Session::events_after` |
| `src/session.rs` | `Session::focus` | `L521: replace && with \|\| in Session::focus`<br>`L521: replace && with \|\| in Session::focus` |
| `src/session.rs` | `Session::started` | `L607: delete match arm SessionState::Exited in Session::started` |
| `src/state.rs` | `State::check_queued` | `L1133: delete match arm Phase::Live(live) in State::check_queued`<br>`L1134: replace match guard prompt.is_empty() with false in State::check_queued` |
| `src/state.rs` | `State::closed` | `L1322: replace State::closed -> bool with false` |
| `src/state.rs` | `State::fill` | `L736: delete match arm EventKind::SteeredPromptTaken{prompt} \| EventKind::SteeredPromptDropped{prompt} in State::fill`<br>`L738: replace != with == in State::fill` |
| `src/state.rs` | `State::start_queued` | `L846: replace + with - in State::start_queued`<br>`L846: replace + with * in State::start_queued` |
| `src/state.rs` | `check_form` | `L1451: replace match guard string.enum_values.is_none() with true in check_form` |
| `src/state.rs` | `check_prompt` | `L1395: delete match arm ContentBlock::Audio(_) in check_prompt` |
| `src/transcript.rs` | `page` | `L191: replace + with - in page` |
| `src/worktree.rs` | `Worktree::close` | `L959: replace Worktree::close with ()` |
| `src/worktree.rs` | `restore` | `L478: replace && with \|\| in restore` |

## Smallest covering set

A greedy set cover over the 462 caught mutants, picking at each step the test that catches the most mutants not yet caught, with ties going to unit tests and then to the earlier name. 64 tests catch every caught mutant. Greedy cover is an upper bound on the smallest set and need not be the only set of its size.

| # | Test | Binary | New catches | Total catches |
| ---: | --- | --- | ---: | ---: |
| 1 | `archiving_removes_the_worktree_and_keeps_the_branch_and_the_conversation` | `worktrees` | 115 | 115 |
| 2 | `a_message_wakes_a_parked_session_with_the_senders_environment` | `agents` | 43 | 113 |
| 3 | `an_approved_request_starts_the_session_it_describes` | `agents` | 26 | 93 |
| 4 | `read_transcript_pages_through_any_sessions_transcript` | `agents` | 25 | 88 |
| 5 | `a_delete_compares_the_repositorys_sessions_again` | `worktrees` | 21 | 83 |
| 6 | `closing_the_focusing_connection_ends_its_focus` | `daemon` | 20 | 87 |
| 7 | `stop_holds_the_queue_and_drops_a_prompt_the_agent_has_not_taken` | `daemon` | 18 | 77 |
| 8 | `an_unarchived_imported_session_resumes_without_replay` | `import` | 17 | 98 |
| 9 | `directory_watch_lists_a_directory_once_it_exists` | `daemon` | 13 | 38 |
| 10 | `cancel_withdraws_an_elicitation` | `daemon` | 10 | 70 |
| 11 | `a_worktree_whose_registration_was_removed_is_removed_only_with_discard` | `worktrees` | 10 | 66 |
| 12 | `a_workspace_removed_during_a_creation` | `worktrees` | 9 | 65 |
| 13 | `state::tests::a_settled_config_sets_only_the_settings_the_agent_accepted` | `unit` | 8 | 12 |
| 14 | `a_queued_prompt_waits_for_a_branch_move_in_its_checkout` | `worktrees` | 8 | 74 |
| 15 | `a_cancelled_turn_is_not_unseen_and_an_error_outlasts_a_finish` | `daemon` | 7 | 75 |
| 16 | `a_daemon_whose_sessions_are_parked_exits` | `daemon` | 6 | 74 |
| 17 | `file_watch_follows_edits_replacement_and_recreation` | `daemon` | 6 | 37 |
| 18 | `queued_prompts_are_edited_and_removed_until_they_are_sent` | `daemon` | 6 | 70 |
| 19 | `importing_again_adds_only_new_sessions` | `import` | 6 | 54 |
| 20 | `oxd_stop_returns_once_an_idle_daemon_has_stopped` | `daemon` | 5 | 22 |
| 21 | `a_move_keeps_an_ignored_file_it_would_overwrite` | `worktrees` | 5 | 40 |
| 22 | `session::tests::parkability_holds_until_a_lapse` | `unit` | 4 | 5 |
| 23 | `state::tests::an_elicitation_awaits_input_unless_a_permission_request_is_open` | `unit` | 4 | 22 |
| 24 | `state::tests::fill_closes_the_turn_and_the_permission_request_a_stop_left_open` | `unit` | 4 | 10 |
| 25 | `transcript::tests::events_fold_into_one_entry_each_keyed_by_their_first_event` | `unit` | 4 | 19 |
| 26 | `prompt_content_reaches_the_agent` | `daemon` | 4 | 64 |
| 27 | `workspaces_and_sessions_survive_a_restart` | `daemon` | 4 | 83 |
| 28 | `a_worktree_deleted_outside_ox_is_unregistered` | `worktrees` | 4 | 72 |
| 29 | `state::tests::a_steered_prompt_pending_at_a_turn_end_runs_before_the_queue` | `unit` | 3 | 24 |
| 30 | `a_failed_worktree_leaves_a_branch_another_worktree_checked_out` | `worktrees` | 3 | 60 |
| 31 | `daemon::tests::only_clients_and_live_sessions_keep_the_daemon_running` | `unit` | 2 | 5 |
| 32 | `state::tests::a_peer_message_starts_a_turn_or_steers_with_its_sender_and_a_header_for_the_agent` | `unit` | 2 | 12 |
| 33 | `state::tests::nothing_follows_session_closed` | `unit` | 2 | 11 |
| 34 | `state::tests::queued_prompts_are_edited_in_place_and_removed_by_id` | `unit` | 2 | 10 |
| 35 | `store::tests::a_deleted_session_leaves_its_agent_session_id_remembered` | `unit` | 2 | 15 |
| 36 | `a_loaded_agent_gets_new_permission_request_ids` | `daemon` | 2 | 78 |
| 37 | `a_setting_the_agent_fails_rejects_the_first_prompt` | `daemon` | 2 | 67 |
| 38 | `version_mismatch` | `daemon` | 2 | 24 |
| 39 | `a_git_directory_git_rejects_is_refused_with_gits_reason` | `worktrees` | 2 | 37 |
| 40 | `submodule_commits_on_no_remote_branch_refuse_the_delete` | `worktrees` | 2 | 62 |
| 41 | `state::tests::a_first_prompt_is_recorded_while_starting_and_runs_once_started` | `unit` | 1 | 12 |
| 42 | `state::tests::a_late_rejection_leaves_the_next_prompt_running` | `unit` | 1 | 11 |
| 43 | `state::tests::a_refused_send_leaves_the_prompt_queued` | `unit` | 1 | 6 |
| 44 | `state::tests::a_rejected_prompt_holds_the_queue` | `unit` | 1 | 17 |
| 45 | `state::tests::commands_replace_the_catalog_including_on_resume_without_replay` | `unit` | 1 | 3 |
| 46 | `state::tests::fill_cancels_an_open_elicitation_and_ids_continue_past_it` | `unit` | 1 | 10 |
| 47 | `state::tests::steered_prompts_the_agent_does_not_take_are_dropped` | `unit` | 1 | 17 |
| 48 | `store::tests::a_deleted_session_takes_its_worktree_and_events` | `unit` | 1 | 20 |
| 49 | `store::tests::a_schema_1_database_gives_each_session_the_config_it_last_reported` | `unit` | 1 | 12 |
| 50 | `a_request_that_cannot_be_started_is_refused_without_asking` | `agents` | 1 | 69 |
| 51 | `a_daemon_with_a_running_turn_refuses_to_stop` | `daemon` | 1 | 67 |
| 52 | `a_session_whose_agent_reports_no_background_work_is_not_parked` | `daemon` | 1 | 77 |
| 53 | `a_stopped_daemon_stores_the_end_of_the_turn_it_interrupted` | `daemon` | 1 | 82 |
| 54 | `a_turn_the_agent_starts_runs_until_the_agent_is_idle` | `daemon` | 1 | 61 |
| 55 | `agent_gets_ox_instructions` | `daemon` | 1 | 60 |
| 56 | `delete_while_starting` | `daemon` | 1 | 59 |
| 57 | `directory_watch_follows_changes` | `daemon` | 1 | 35 |
| 58 | `exhausted_descriptors_do_not_stop_the_daemon` | `daemon` | 1 | 55 |
| 59 | `oxd_stop_succeeds_with_no_daemon_running` | `daemon` | 1 | 5 |
| 60 | `subscriptions_to_one_session_are_independent` | `daemon` | 1 | 62 |
| 61 | `a_branch_another_worktree_has_checked_out_is_kept` | `worktrees` | 1 | 67 |
| 62 | `a_session_imported_during_a_branch_move_refuses_loads_until_it_ends` | `worktrees` | 1 | 71 |
| 63 | `a_subdirectory_workspace_runs_in_the_same_subdirectory` | `worktrees` | 1 | 54 |
| 64 | `a_worktree_whose_repository_moved_is_removed_only_with_discard` | `worktrees` | 1 | 69 |

## Deletion pool

The 186 tests outside the covering set. Every mutant one of them caught, a test in the covering set also caught. *Sole timeout* counts the timed-out mutants on which this test reacted and no covering-set test did.

| Test | Binary | Caught | Timeouts | Sole timeout |
| --- | --- | ---: | ---: | ---: |
| `store::tests::events_round_trip_in_order_across_batches` | `unit` | 22 | 0 |  |
| `state::tests::turns_that_end_other_than_normally_hold_the_queue_until_one_is_sent_now` | `unit` | 20 | 8 |  |
| `store::tests::an_imported_session_round_trips_as_archived_with_its_events` | `unit` | 19 | 0 |  |
| `store::tests::unseen_is_saved_replaced_and_deleted_with_the_session` | `unit` | 18 | 0 |  |
| `state::tests::transitions_record_the_changes_to_the_saved_session` | `unit` | 17 | 5 |  |
| `store::tests::removing_a_workspace_deletes_its_sessions_without_remembering_them` | `unit` | 17 | 0 |  |
| `store::tests::sessions_round_trip_in_the_order_they_were_created` | `unit` | 17 | 0 |  |
| `state::tests::a_withdrawal_ends_only_the_elicitation_whose_receiver_closed` | `unit` | 16 | 4 |  |
| `state::tests::cancelling_holds_the_queue_until_a_prompt_is_sent` | `unit` | 16 | 8 |  |
| `store::tests::every_schema_fixture_opens_and_reads_back` | `unit` | 16 | 0 |  |
| `transcript::tests::a_page_stops_at_the_budget_and_names_the_earlier_entries` | `unit` | 16 | 0 |  |
| `store::tests::a_forgotten_worktree_leaves_its_session_without_one` | `unit` | 15 | 0 |  |
| `store::tests::a_saved_queue_comes_back_with_its_session_and_is_deleted_with_it` | `unit` | 15 | 0 |  |
| `store::tests::a_session_round_trips_with_its_worktree` | `unit` | 15 | 0 |  |
| `store::tests::archiving_is_saved_and_deleted_with_the_session` | `unit` | 15 | 0 |  |
| `transcript::tests::an_entry_over_the_budget_is_cut` | `unit` | 15 | 0 |  |
| `state::tests::a_session_is_quiet_when_the_agent_reports_no_background_work` | `unit` | 14 | 6 |  |
| `store::tests::saved_title_agent_session_id_and_config_read_back` | `unit` | 14 | 0 |  |
| `state::tests::a_moving_session_refuses_turns_and_loads_and_holds_its_queue` | `unit` | 13 | 8 |  |
| `state::tests::a_restored_session_is_exited_and_loads_its_replay_as_history` | `unit` | 13 | 6 |  |
| `state::tests::state_follows_the_turn_and_its_permission_requests` | `unit` | 13 | 6 |  |
| `state::tests::a_prompt_during_a_turn_is_steered_and_taken_when_the_agent_accepts_it` | `unit` | 12 | 6 |  |
| `state::tests::a_rejected_prompt_ends_the_turn_with_its_error` | `unit` | 12 | 6 |  |
| `state::tests::queued_prompts_start_one_turn_each_in_order_after_normal_ends` | `unit` | 12 | 8 |  |
| `store::tests::a_deleted_session_is_gone_and_its_number_is_not_reused` | `unit` | 12 | 0 |  |
| `store::tests::a_failed_change_leaves_the_others_unwritten` | `unit` | 12 | 0 |  |
| `store::tests::a_session_whose_worktree_row_fails_is_not_saved` | `unit` | 12 | 0 |  |
| `state::tests::a_rejected_prompt_after_the_agent_reported_work_waits_for_idle` | `unit` | 11 | 6 |  |
| `state::tests::a_turn_ends_once` | `unit` | 11 | 6 |  |
| `state::tests::the_resume_snapshot_gives_the_session_its_background_work` | `unit` | 11 | 1 |  |
| `state::tests::the_resume_snapshot_sets_the_state_without_recording_a_turn` | `unit` | 11 | 2 |  |
| `store::tests::workspaces_round_trip_in_the_order_they_were_added` | `unit` | 11 | 0 |  |
| `state::tests::a_peer_message_after_a_cancelled_turn_leaves_the_queue_held` | `unit` | 10 | 8 |  |
| `state::tests::permission_request_ids_continue_after_the_history` | `unit` | 10 | 1 |  |
| `store::tests::a_workspace_path_is_saved_once` | `unit` | 10 | 0 |  |
| `store::tests::events_of_a_deleted_session_are_dropped` | `unit` | 10 | 0 |  |
| `state::tests::a_prompt_queued_on_an_idle_session_starts_at_once` | `unit` | 9 | 5 |  |
| `state::tests::an_answered_prompt_that_was_rejected_after_its_turn_ended_changes_nothing` | `unit` | 9 | 6 |  |
| `state::tests::cancelling_the_turn_leaves_an_elicitation_open` | `unit` | 9 | 4 |  |
| `state::tests::the_agent_starts_and_ends_turns_of_its_own` | `unit` | 9 | 2 |  |
| `state::tests::a_repeated_idle_snapshot_after_a_prompt_leaves_the_turn_running` | `unit` | 8 | 6 |  |
| `store::tests::a_database_from_before_versions_opens_at_the_latest_version_with_its_rows` | `unit` | 8 | 0 |  |
| `state::tests::a_load_that_fails_leaves_the_session_loadable` | `unit` | 7 | 1 |  |
| `state::tests::exit_and_close_end_the_turn_and_its_permission_requests` | `unit` | 7 | 6 |  |
| `state::tests::exit_park_and_close_cancel_an_elicitation` | `unit` | 7 | 6 |  |
| `transcript::tests::each_kind_of_turn_end_has_its_line` | `unit` | 7 | 0 |  |
| `state::tests::a_first_prompt_ends_when_the_agent_exits_while_starting` | `unit` | 6 | 1 |  |
| `state::tests::a_load_that_fails_partway_through_its_replay_records_none_of_it` | `unit` | 6 | 1 |  |
| `state::tests::a_move_begins_only_on_an_idle_or_exited_session` | `unit` | 6 | 6 |  |
| `state::tests::session_info_updates_set_clear_and_keep_the_title` | `unit` | 6 | 1 |  |
| `store::tests::a_new_database_has_every_table_at_the_latest_version` | `unit` | 6 | 0 |  |
| `session::tests::a_lapse_while_the_park_waits_for_the_lifecycle_restarts_the_delay` | `unit` | 5 | 4 |  |
| `state::tests::a_session_with_a_config_keeps_it_when_the_agent_reports_another` | `unit` | 5 | 1 |  |
| `state::tests::an_appended_event_carries_the_time_it_was_recorded` | `unit` | 5 | 1 |  |
| `state::tests::exit_ends_a_turn_the_agent_started` | `unit` | 5 | 1 |  |
| `state::tests::states_other_than_idle_are_work` | `unit` | 5 | 2 |  |
| `import::tests::user_messages_start_turns_that_end_before_the_next` | `unit` | 4 | 1 |  |
| `state::tests::a_failed_start_forgets_the_worktree` | `unit` | 4 | 1 |  |
| `state::tests::a_restored_session_that_never_started_holds_its_first_prompt_and_cannot_load` | `unit` | 4 | 0 |  |
| `state::tests::a_session_with_a_history_loads_without_duplicating_it` | `unit` | 4 | 2 |  |
| `state::tests::fill_leaves_a_finished_history_as_it_is` | `unit` | 4 | 0 |  |
| `state::tests::forms_a_client_cannot_render_are_refused` | `unit` | 4 | 0 |  |
| `state::tests::reported_config_reads_model_effort_and_mode_by_category` | `unit` | 4 | 0 |  |
| `state::tests::session_closed_is_not_logged` | `unit` | 4 | 1 |  |
| `state::tests::unseen_reads_how_turns_and_agents_ended` | `unit` | 4 | 0 |  |
| `session::tests::a_lapse_that_reverts_before_the_park_task_looks_restarts_the_delay` | `unit` | 3 | 4 |  |
| `state::tests::a_restored_session_holds_no_events_until_it_is_filled` | `unit` | 3 | 0 |  |
| `state::tests::a_restored_session_whose_history_has_a_prompt_loads_without_replay` | `unit` | 3 | 0 |  |
| `state::tests::parking_exits_a_quiet_session_and_records_it` | `unit` | 3 | 1 |  |
| `store::tests::a_database_of_a_newer_schema_is_refused` | `unit` | 3 | 0 |  |
| `import::tests::updates_without_a_prompt_end_no_turn` | `unit` | 2 | 1 |  |
| `session::tests::a_parkable_session_is_parked_once_the_delay_passes` | `unit` | 2 | 4 |  |
| `session::tests::a_session_is_parked_only_while_it_stays_parkable` | `unit` | 2 | 4 |  |
| `state::tests::an_untitled_sender_is_named_by_its_id_alone` | `unit` | 2 | 0 |  |
| `daemon::tests::a_change_restarts_the_delay_and_busy_time_does_not_count` | `unit` | 1 | 0 |  |
| `daemon::tests::idling_for_the_delay_resolves` | `unit` | 1 | 0 |  |
| `state::tests::only_an_exited_session_with_an_agent_session_loads` | `unit` | 1 | 1 |  |
| `transcript::tests::an_empty_history_has_no_entries` | `unit` | 0 | 0 |  |
| `a_message_loads_a_session_restored_after_a_restart` | `agents` | 104 | 32 |  |
| `a_message_to_an_idle_session_starts_a_turn_with_its_sender` | `agents` | 74 | 28 |  |
| `list_sessions_lists_the_sessions_one_can_message` | `agents` | 73 | 35 |  |
| `a_message_to_a_busy_session_steers_its_turn` | `agents` | 72 | 28 |  |
| `messages_to_sessions_that_cannot_take_them_are_refused` | `agents` | 72 | 37 |  |
| `a_declined_request_creates_nothing` | `agents` | 71 | 30 |  |
| `a_request_in_the_workspaces_checkout_names_its_directory` | `agents` | 63 | 26 |  |
| `list_harnesses_lists_the_config` | `agents` | 59 | 26 |  |
| `focusing_one_exited_session_after_another_loads_both` | `daemon` | 114 | 27 |  |
| `a_held_queue_survives_a_restart` | `daemon` | 106 | 28 |  |
| `loading_a_restored_session_keeps_its_stored_history` | `daemon` | 103 | 29 |  |
| `a_session_resumed_while_its_agent_is_working_runs_until_the_agent_is_idle` | `daemon` | 98 | 29 |  |
| `a_parked_session_loads_again` | `daemon` | 93 | 29 |  |
| `a_history_that_a_crash_left_open_ends_its_turn` | `daemon` | 91 | 20 |  |
| `loading_a_restored_session_recovers_missing_history_from_the_agent` | `daemon` | 89 | 29 |  |
| `a_session_with_background_work_is_not_parked` | `daemon` | 88 | 27 |  |
| `unseen_outcomes_survive_a_restart` | `daemon` | 84 | 28 |  |
| `a_viewed_session_is_not_parked` | `daemon` | 83 | 28 |  |
| `a_loaded_session_sends_its_recorded_config_again` | `daemon` | 80 | 29 |  |
| `an_archived_session_that_never_started_keeps_its_history_across_a_restart` | `daemon` | 80 | 19 |  |
| `a_turn_that_ends_out_of_focus_is_unseen_until_the_session_is_focused` | `daemon` | 78 | 19 |  |
| `a_deleted_session_loses_its_history` | `daemon` | 74 | 17 |  |
| `a_turn_after_the_resume_answer_is_live` | `daemon` | 74 | 27 |  |
| `loading_a_session_whose_agent_exited_keeps_its_history` | `daemon` | 74 | 29 |  |
| `exited_sessions_do_not_keep_the_daemon_alive` | `daemon` | 73 | 24 |  |
| `a_new_sessions_first_prompt_shows_before_its_agent_starts` | `daemon` | 70 | 18 |  |
| `set_config_sets_only_its_settings_and_the_next_prompt_runs_with_them` | `daemon` | 70 | 25 |  |
| `queued_prompts_are_sent_when_turns_end_with_no_client_connected` | `daemon` | 69 | 25 |  |
| `watch_sends_only_sessions_whose_info_changed` | `daemon` | 69 | 28 |  |
| `a_workspace_is_removed_once_its_sessions_are_archived` | `daemon` | 67 | 40 | 7 |
| `elicitation_round_trip` | `daemon` | 67 | 23 |  |
| `session_info_updates_set_the_title` | `daemon` | 67 | 25 |  |
| `an_idle_unviewed_session_is_parked_and_its_agent_exits` | `daemon` | 66 | 14 |  |
| `agent_sets_title_through_ox_mcp_server` | `daemon` | 65 | 25 |  |
| `permission_across_disconnect` | `daemon` | 65 | 24 |  |
| `reattach_mid_turn` | `daemon` | 65 | 25 |  |
| `unsubscribe_ends_a_watch` | `daemon` | 65 | 23 |  |
| `a_new_session_shows_its_config_throughout_and_sets_it_before_its_first_prompt` | `daemon` | 64 | 17 |  |
| `a_refused_setting_is_named_and_the_accepted_ones_are_kept` | `daemon` | 64 | 15 |  |
| `concurrent_set_config_requests_keep_every_setting` | `daemon` | 63 | 15 |  |
| `permission_after_turn_returns_session_to_idle` | `daemon` | 63 | 25 |  |
| `unsubscribe_ends_a_subscription` | `daemon` | 63 | 25 |  |
| `a_late_rejection_does_not_end_the_next_prompt` | `daemon` | 62 | 23 |  |
| `cancel_with_pending_permission` | `daemon` | 62 | 23 |  |
| `focus_loads_only_an_exited_session_that_started` | `daemon` | 62 | 21 |  |
| `vanished_client` | `daemon` | 62 | 24 |  |
| `a_prompt_sent_during_a_turn_is_taken_into_it` | `daemon` | 61 | 25 |  |
| `prompt_content_the_agent_does_not_accept_is_rejected` | `daemon` | 61 | 25 |  |
| `a_stopping_daemon_sends_the_rest_of_each_subscription` | `daemon` | 60 | 19 |  |
| `slow_subscriber_receives_every_event` | `daemon` | 60 | 17 |  |
| `a_prompt_the_agent_rejects_ends_its_turn_with_the_error` | `daemon` | 59 | 23 |  |
| `a_turn_ends_with_the_stop_reason_the_agent_reports` | `daemon` | 59 | 25 |  |
| `lifetime` | `daemon` | 59 | 15 |  |
| `delete_ends_the_turn_and_its_permission_request` | `daemon` | 58 | 27 |  |
| `agent_exit` | `daemon` | 57 | 25 |  |
| `delete_ends_other_clients_subscriptions` | `daemon` | 54 | 18 |  |
| `a_first_prompt_ends_when_the_agent_does_not_start` | `daemon` | 53 | 13 |  |
| `a_session_runs_in_its_workspace` | `daemon` | 53 | 16 |  |
| `sigterm_stops_agents_and_removes_the_socket` | `daemon` | 53 | 14 |  |
| `delete_stops_the_agent` | `daemon` | 52 | 16 |  |
| `an_agent_without_session_capabilities_cannot_run_sessions` | `daemon` | 51 | 13 |  |
| `a_first_prompt_with_content_beyond_text_and_links_is_refused` | `daemon` | 34 | 5 |  |
| `workspace_add_refuses_what_is_not_a_new_directory` | `daemon` | 33 | 8 |  |
| `focusing_an_unknown_session_is_refused` | `daemon` | 26 | 2 |  |
| `an_idle_daemon_stops_when_asked_before_hello` | `daemon` | 25 | 1 |  |
| `single_instance_and_on_demand_start` | `daemon` | 24 | 5 |  |
| `socket_is_private` | `daemon` | 15 | 1 |  |
| `importing_adds_each_session_archived_with_its_history` | `import` | 76 | 12 |  |
| `removing_a_workspace_of_archived_sessions_lets_them_be_imported_again` | `import` | 57 | 14 |  |
| `a_deleted_imported_session_is_not_imported_again` | `import` | 55 | 11 |  |
| `a_transcript_that_fails_is_skipped` | `import` | 49 | 10 |  |
| `uncommitted_changes_refuse_the_delete_until_it_discards_them` | `worktrees` | 81 | 37 |  |
| `a_worktree_session_that_never_started_keeps_its_first_prompt_across_a_restart` | `worktrees` | 79 | 20 |  |
| `a_locked_worktree_fails_the_delete_and_the_session_loads_again` | `worktrees` | 78 | 39 |  |
| `deleting_an_archived_session_removes_it_and_applies_the_branch_rule` | `worktrees` | 77 | 37 |  |
| `a_worktree_session_survives_a_restart_and_loads_only_while_its_directory_exists` | `worktrees` | 75 | 32 |  |
| `a_turn_end_compares_again` | `worktrees` | 74 | 33 |  |
| `a_worktree_that_cannot_be_added_is_undone` | `worktrees` | 74 | 30 |  |
| `a_branch_merged_elsewhere_is_deleted_and_one_with_its_own_commits_is_kept` | `worktrees` | 73 | 42 |  |
| `a_clean_delete_removes_the_worktree_and_the_branch` | `worktrees` | 73 | 41 |  |
| `a_branch_is_kept_until_its_commits_are_pushed` | `worktrees` | 72 | 42 |  |
| `a_session_whose_workspace_subdirectory_is_gone_is_deleted` | `worktrees` | 72 | 42 |  |
| `a_comparison_ref_survives_a_restart_and_is_compared_unasked` | `worktrees` | 71 | 30 |  |
| `removing_a_workspace_waits_for_an_unarchive_under_way` | `worktrees` | 68 | 40 |  |
| `a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn` | `worktrees` | 67 | 42 |  |
| `a_session_runs_in_a_worktree_on_an_existing_branch` | `worktrees` | 67 | 38 |  |
| `a_move_compares_the_repositorys_sessions_again` | `worktrees` | 66 | 47 |  |
| `archiving_a_running_session_ends_its_turn` | `worktrees` | 65 | 24 |  |
| `archiving_refuses_unsaved_work_unless_it_is_discarded` | `worktrees` | 64 | 39 |  |
| `unarchiving_again_finishes_when_the_worktree_is_already_back` | `worktrees` | 64 | 37 |  |
| `a_worktree_is_listed_before_its_session_and_removed_with_it` | `worktrees` | 61 | 25 |  |
| `a_worktree_sessions_first_prompt_runs_in_the_worktree_once_added` | `worktrees` | 60 | 28 |  |
| `a_comparison_is_refreshed_on_request` | `worktrees` | 59 | 30 |  |
| `a_delete_during_startup_waits_for_the_worktree` | `worktrees` | 59 | 39 |  |
| `commits_on_no_branch_refuse_the_delete` | `worktrees` | 57 | 28 |  |
| `unarchiving_needs_a_branch_git_can_check_out` | `worktrees` | 57 | 39 |  |
| `a_worktree_that_cannot_be_added_ends_the_first_prompts_turn` | `worktrees` | 56 | 27 |  |
| `a_session_runs_in_a_new_worktree_on_a_new_branch` | `worktrees` | 54 | 29 |  |
| `git_ignores_the_clients_repository_variables` | `worktrees` | 54 | 25 |  |
| `a_linked_worktree_workspace_names_the_worktree_after_itself` | `worktrees` | 53 | 27 |  |
| `a_new_session_publishes_its_comparison` | `worktrees` | 49 | 30 |  |
| `workspaces_say_whether_they_are_in_a_git_working_tree` | `worktrees` | 44 | 6 |  |
| `a_move_keeps_uncommitted_and_untracked_work` | `worktrees` | 37 | 43 |  |
| `moving_a_checked_out_branch_moves_its_checkout` | `worktrees` | 35 | 34 |  |
| `a_branch_no_checkout_has_moves_only_from_where_it_is_expected` | `worktrees` | 32 | 41 |  |
| `a_request_git_cannot_satisfy_is_refused_with_nothing_created` | `worktrees` | 32 | 18 |  |
| `branches_are_refused_outside_a_working_tree` | `worktrees` | 32 | 7 |  |
| `branches_name_the_branch_the_checkout_is_on` | `worktrees` | 32 | 18 |  |

## Conclusion

By this measure, one test is worthless:
`transcript::tests::an_empty_history_has_no_entries` failed on no mutant.

Judged one test at a time, 210 tests (84%) look redundant, because another
test failed on every mutant they caught. That count overstates the
redundancy: two tests that catch the same mutants each look redundant. The
set cover gives a tighter number. 64 tests (26%) catch all 462 caught
mutants, and the other 186 (74%) form the deletion pool: 78 unit tests,
59 `daemon`, 37 `worktrees`, 8 `agents` and 4 `import`. Every pool test can go
together without losing a catch from this run.

One pool test should stay anyway.
`daemon::a_workspace_is_removed_once_its_sessions_are_archived` caught nothing
by the measure's definition. It was the only test to react to 7 timed-out
mutants in `Daemon::remove_workspace` and `Daemon::workspace_sessions`, where
it hung. On those mutants the `import` and `worktrees` binaries never ran.

The 43 missed mutants are the unprotected code. Most are in
`daemon.rs` (11 mutants across eight functions, among them `Daemon::flush`,
`Daemon::is_idle`, `Registry::refuse_moving` and `now`), `state.rs` (9, six of
them in queued and steered prompts: `check_queued`, `start_queued` and
`fill`) and `directories.rs` (6, all in the `Changes` bookkeeping). Some of these may be equivalent mutants,
which change no observable behavior; this run does not tell them apart.

### Limits of the measure

- A test gets credit only for behavior that a `cargo-mutants` operator
  changes: replaced function bodies, swapped operators, deleted match arms,
  and replaced match guards. It gets none for behavior those operators never
  touch: SQL text and schema, string and numeric constants, struct contents,
  the order of concurrent work, and code outside `crates/oxd/src`, such as
  `ox-protocol`, `acp-bridge` and the fake agent. A test that guards only that
  kind of behavior shows up here as worthless or in the deletion pool.
- Failures cascade. 59 caught mutants each failed 50 or more tests (up to
  174), usually by breaking startup or session launch. These cascades make
  integration tests look redundant with one another without their asserting
  the same thing. The median caught mutant failed 5 tests.
- Timeouts (106, 14% of mutants) are excluded, and their logs stop at the
  hanging binary. A pool test may be the only detector of a hang this run
  could not see.
- Greedy set cover is an upper bound on the smallest covering set, and it is
  not unique. Whether a test lands in the pool depends on tie-breaks, which
  prefer unit tests. A different cover could keep a different 64.
- Each mutant ran once. A flaky failure would count as a catch. The unmutated
  runs under load did not show any.

Every count here can be recomputed from `/tmp/ox-mutants-out` (the
`cargo-mutants` outputs, `parsed.json` and `analysis.json`, which list each
test's caught mutants), for as long as `/tmp` keeps it.

## Triage of the missed mutants

Each missed mutant was read against the code at `a33cf04` to see what it
would break. They fall into three groups.

### Behavior no test checks (24 mutants)

Each of these is a working rule of `oxd` that could break without a test
failing.

| Behavior | Mutants | Test that would cover it |
| --- | --- | --- |
| A request sent before `hello` is refused | `conn.rs:107` | Integration: send a request first, expect `INVALID_REQUEST` |
| A message to a session whose agent fails to start reports "the agent did not start" | `session.rs:607` | Integration: fake agent that exits while starting |
| A queued prompt the agent rejects ends its turn | `state.rs:846` (×2) | Unit: start a queued prompt, reject it, expect `TurnEnded` |
| A queued prompt is checked against the running agent's capabilities; an empty one is refused while no agent runs | `state.rs:1133`, `state.rs:1134` | Unit: queue an image for an agent without image support; queue an empty prompt on an exited session |
| Reading a history back does not drop a steered prompt the agent already took | `state.rs:736`, `state.rs:738` | Unit: `fill` a history with `PromptSteered` then `SteeredPromptTaken`, expect no `SteeredPromptDropped` |
| No session is created in a checkout during a branch move | `daemon.rs:109` | Integration: create a session while a move holds the checkout |
| The daemon does not exit on its idle timer while a client is connected or a session runs | `daemon.rs:223` | Integration with `idle_exit_ms` and a connected client |
| A watch reports a removed workspace | `daemon.rs:511` (×2) | Integration: remove a workspace, expect `WorkspaceRemoved` |
| `start_session` with an explicit `workspace` path picks that workspace | `daemon.rs:917`, `daemon.rs:1465` | Integration: two workspaces, request the second by path |
| A branch move refuses only paths that collide with ignored files | `branches.rs:285` (×2) | Unit: `collides` with a nested path that does not collide, and with an ignored file whose name prefixes a directory |
| Restoring a worktree is skipped only when the directory is that branch's worktree | `worktree.rs:478` | Integration: a plain directory at the worktree's path |
| A streamed file whose directory is created or deleted later keeps updating | `directories.rs:164`, `files.rs:48` (×2) | Integration: stream a file in a missing directory, then create it |
| Empty user messages in an imported Claude history do not split turns | `import.rs:87` | Unit: import with an empty user message |
| An agent that accepts audio gets audio; a form with an enum string property is refused | `state.rs:1395`, `state.rs:1451` | Unit: `check_prompt` and `check_form` cases |
| A transcript page counts the separators between entries against its budget | `transcript.rs:191` | Unit: entries that fill the budget exactly |

### Leaks, busy loops and races (14 mutants)

These break something real, but the effect is a leaked task or watch, wasted
CPU, or a race window that no test can trigger. A test would need
instrumentation `oxd` does not have.

| Effect of the mutant | Mutants |
| --- | --- |
| A subscription loops at full CPU when it has no new event | `session.rs:574` |
| A subscription's task outlives its closed session | `conn.rs:308` |
| A directory watch is never released, or its waiter spins, or re-reads once more | `directories.rs:169`, `:175`, `:181`, `:186` (×2) |
| A worktree's comparison keeps running after it closes | `worktree.rs:959` |
| Events recorded just before shutdown may not reach the database | `daemon.rs:248` |
| A workspace removal racing a session delete or import goes ahead | `daemon.rs:597`, `daemon.rs:613` |
| A lifecycle operation proceeds on a session deleted while it waited | `state.rs:1322` |
| A config change blocks the connection's later requests until the agent answers | `conn.rs:199` |
| `oxd --stop` reports "not running" for any connection error | `main.rs:250` |

`daemon.rs:248` (shutdown flush) and `conn.rs:199` (config change in its own
task) are the two of these worth a test. Both can be made deterministic with
the fake agent, which can delay its answer.

### No observable difference (5 mutants)

| Mutant | Why nothing changes |
| --- | --- |
| `session.rs:521` (×2), `Session::focus` | `load_if_exited` and `resume` repeat the same checks. The mutant adds a log line at most. |
| `main.rs:171`, socket removal | Any other error from removing the socket makes the `bind` that follows fail too. |
| `daemon.rs:1457` (×2), `now` | No test reads a timestamp's value. A broken clock would show only in the UI's times. |


The `SessionSetConfig` arm of `Daemon::handle` (`daemon.rs:348`) is never
reached: `conn.rs` handles that method before it gets there.

The triage reads `a33cf04`. Since then, `15a614c` removed `State::fill` and
`Daemon::flush`, so `state.rs:736`, `state.rs:738` and `daemon.rs:248` no
longer apply. That leaves 22 behaviors no test checks.
