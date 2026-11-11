//! The gate's rules, tested without a helper or a screen. `policy_cases`, `target_cases`
//! and `command_cases` are ViewCode's cases (`computerUsePolicy.test.ts`,
//! `computerUseTargets.test.ts`, `computerUseCommand.test.ts`), one for one; `gate`
//! covers the decisions the service makes from them.

use super::command::{acp_computer_use_allow_once_option, auto_approves_computer_use_command, is_computer_use_screenshot_read, is_plain_computer_use_command, AcpPermissionRules, Command, SessionGrant};
use super::policy::*;
use super::protocol::{DispatchTarget, Dispatched, DriverElement, DriverFailure, DriverWindow, ElementIdentity, FailureKind, FailureReason, MouseButton, Rect};
use super::targets::{NewShot, RefLookup, RefRetired, ShotLookup, ShotRetired, ThreadTargets, WindowRecord};
use serde_json::{json, Value};
use std::collections::HashSet;

fn app(app: &'static str, app_identifier: Option<&'static str>) -> AppWindow<'static> {
    AppWindow { app, app_identifier, title: None, pid: None }
}

fn driver_window(handle: &str, app: &str, pid: u32, title: &str) -> DriverWindow {
    DriverWindow { handle: handle.into(), app: app.into(), pid, title: title.into(), focused: true, bounds: None, app_identifier: None, kind: None }
}

fn element(handle: &str, role: &str, label: &str) -> DriverElement {
    DriverElement { handle: handle.into(), role: role.into(), label: label.into(), value: None, section: None, instance: None, enabled: true, focused: false }
}

fn identity(role: &str, label: &str) -> ElementIdentity {
    ElementIdentity { role: role.into(), label: label.into() }
}

mod policy_cases {
    use super::*;

    #[test]
    fn refuses_a_browser_window_showing_the_app_but_not_pages_that_only_mention_it() {
        let browser = |app: &str, title: &str| is_denylisted_app(AppWindow { app, app_identifier: None, title: Some(title), pid: None });
        assert!(browser("Google Chrome", "ViewCode (Alpha) - Google Chrome"));
        assert!(browser("Safari", "ViewCode"));
        assert!(browser("Arc", "T3 Code (Dev) \u{2014} Work"));
        assert!(!browser("Google Chrome", "GitHub - CreatorGhost/ViewCode"));
        assert!(!browser("Visual Studio Code", "ViewCode \u{2014} server.ts"));
        // The same rule under this app's own name.
        assert!(browser("Google Chrome", "PandaCode (Alpha) - Google Chrome"));
        assert!(browser("Safari", "PandaCode"));
        assert!(!browser("Google Chrome", "GitHub - example/PandaCode"));
    }

    #[test]
    fn refuses_protected_apps_and_the_app_itself() {
        let refused = [
            // Matched by the `.app` bundle on the executable path, whatever the window's app name says.
            app("Electron", Some("/Applications/T3 Code (Alpha).app/Contents/MacOS/T3 Code (Alpha)")),
            app("ViewCode Helper (Renderer)", Some("/Applications/ViewCode.app/Contents/Frameworks/ViewCode Helper (Renderer).app/Contents/MacOS/ViewCode Helper (Renderer)")),
            app("1Password", Some("/Applications/1Password.app/Contents/MacOS/1Password")),
            app("Settings", Some("/System/Applications/System Settings.app/Contents/MacOS/System Settings")),
            app("Keychain", Some("/System/Applications/Utilities/Keychain Access.app/Contents/MacOS/Keychain Access")),
            // ViewCode's shipped launcher: "ViewCode (Alpha)" with an Electron executable.
            app("ViewCode (Alpha)", Some("/Applications/ViewCode (Alpha).app/Contents/MacOS/Electron")),
            app("ViewCode (Dev)", Some("/tmp/build/Electron.app/Contents/MacOS/Electron")),
            app("ViewCode (Nightly)", None),
            app("T3 Code (Dev)", None),
            // Linux executables, and names alone when no path is known.
            app("bitwarden", Some("/opt/Bitwarden/bitwarden")),
            app("ViewCode", None),
        ];
        assert_eq!(refused.len(), 11);
        for window in refused {
            assert!(is_denylisted_app(window), "{window:?}");
        }
    }

    #[test]
    fn refuses_the_app_under_its_own_name() {
        for window in [
            app(APP_NAME, None),
            app("PandaCode (Dev)", None),
            app("pandacode", Some("/usr/bin/pandacode")),
            app("Renderer", Some("/Applications/PandaCode.app/Contents/MacOS/pandacode")),
            app("WebKit", Some("C:\\Program Files\\PandaCode\\pandacode.exe")),
            app("PandaCode Helper (GPU)", None),
        ] {
            assert!(is_denylisted_app(window), "{window:?}");
        }
        assert!(!is_protected_app(app(APP_NAME, None)), "the app is refused, but it does not block screenshots of windows it overlaps");
    }

    #[test]
    fn allows_ordinary_apps() {
        let allowed = [app("Notes", Some("/System/Applications/Notes.app/Contents/MacOS/Notes")), app("Bambu Studio", Some("/Applications/BambuStudio.app/Contents/MacOS/BambuStudio")), app("Safari", None)];
        assert_eq!(allowed.len(), 3);
        for window in allowed {
            assert!(!is_denylisted_app(window), "{window:?}");
        }
    }

    #[test]
    fn does_not_treat_a_canvas_formatting_description_as_a_format_action() {
        assert!(!is_destructive_target("group", "Format, move, and resize items within the Canvas"));
        assert!(is_destructive_target("group", "Delete all items"));
        assert!(is_destructive_target("group", "Format disk"));
        assert!(is_destructive_target("button", "Format disk"));
        assert!(is_destructive_target("menu item", "Format"));
        assert!(is_destructive_target("unknown", "Format disk"));
    }

    #[test]
    fn flags_committing_labels_and_quit_or_close_chords_only() {
        assert!(is_destructive_target("button", "Place order"));
        assert!(is_destructive_target("button", "Send"));
        assert!(!is_destructive_target("button", "Sender details"));
        assert!(is_destructive_chord("cmd+q"));
        assert!(is_destructive_chord("alt+f4"));
        assert!(is_destructive_chord("cmd+backspace"));
        assert!(!is_destructive_chord("cmd+shift+z"));
        assert!(!is_destructive_chord("q"));
    }

    #[test]
    fn counts_typed_characters_instead_of_showing_them() {
        let detail = describe_input_for_approval(&InputRequest::TypeFocused { window: 1, text: "correct horse".into() }, &ApprovalTarget { app: "Terminal", window_title: "zsh", element: None, hit: Hit::NotApplicable });
        assert_eq!(detail, "Type 13 characters into the focused field in Terminal — \"zsh\"; brings the window to the front");
    }

    #[test]
    fn clips_long_window_titles_and_labels() {
        let title = "t".repeat(300);
        let detail = describe_input_for_approval(&InputRequest::Press { reference: 1 }, &ApprovalTarget { app: "Notes", window_title: &title, element: Some(identity("button", &"l".repeat(300))), hit: Hit::NotApplicable });
        assert!(detail.contains(&format!("\"{}…\"", "l".repeat(79))));
        assert!(detail.contains(&format!("\"{}…\"", "t".repeat(79))));
        assert!(detail.chars().count() < 220);
    }

    #[test]
    fn states_both_points_of_a_drag() {
        let detail = describe_input_for_approval(&InputRequest::Drag { shot: 3, from_x: 1, from_y: 2, to_x: 30, to_y: 40 }, &ApprovalTarget { app: "Bambu Studio", window_title: "Plate 1", element: None, hit: Hit::Nothing });
        assert_eq!(detail, "Drag from (1, 2) to (30, 40) in Bambu Studio — \"Plate 1\" (no labelled control there); brings the window to the front");
    }

    #[test]
    fn typed_and_set_text_never_reaches_an_approval_or_a_debug_line() {
        let secret = "hunter2 🙂";
        let field = Some(identity("text field", "Password"));
        let target = ApprovalTarget { app: "Notes", window_title: "Login", element: field, hit: Hit::NotApplicable };
        let typed = InputRequest::Type { reference: 4, text: secret.into() };
        let set = InputRequest::SetValue { reference: 4, value: "x".into() };
        assert_eq!(describe_input_for_approval(&typed, &target), "Type 9 characters into text field \"Password\" in Notes — \"Login\"");
        assert_eq!(describe_input_for_approval(&set, &target), "Set text field \"Password\" to 1 character in Notes — \"Login\"");
        assert_eq!(format!("{typed:?} {:?}", Request::Input(typed.clone())), "InputRequest(type) Request(type)");
    }

    #[test]
    fn words_every_other_action() {
        let target = |hit| ApprovalTarget { app: "", window_title: " ", element: Some(identity("", "Go")), hit };
        let on = Hit::Element(identity("button", "Done"));
        assert_eq!(describe_input_for_approval(&InputRequest::Key { window: 1, keys: "cmd+s".into() }, &target(Hit::NotApplicable)), "Press cmd+s in an app — (no label)");
        assert_eq!(describe_input_for_approval(&InputRequest::Scroll { reference: 1, dx: 0, dy: -3 }, &target(Hit::NotApplicable)), "Scroll element \"Go\" by (0, -3) in an app — (no label)");
        assert_eq!(describe_input_for_approval(&InputRequest::Click { shot: 1, x: 5, y: 6, button: None, count: Some(2) }, &target(on.clone())), "Double-click at (5, 6) in an app — (no label) (on button \"Done\"); brings the window to the front");
        assert_eq!(describe_input_for_approval(&InputRequest::Click { shot: 1, x: 5, y: 6, button: Some(MouseButton::Right), count: Some(2) }, &target(Hit::Nothing)), "Right-click at (5, 6) in an app — (no label) (no labelled control there); brings the window to the front");
        assert_eq!(describe_input_for_approval(&InputRequest::Move { shot: 1, x: 5, y: 6 }, &target(on.clone())), "Move the pointer to (5, 6) in an app — (no label) (on button \"Done\"); brings the window to the front");
        assert_eq!(describe_input_for_approval(&InputRequest::ScrollAt { shot: 1, x: 5, y: 6, dx: 1, dy: 2 }, &target(on)), "Scroll by (1, 2) at (5, 6) in an app — (no label) (on button \"Done\"); brings the window to the front");
        assert_eq!(describe_input_for_approval(&InputRequest::Focus { window: 1 }, &ApprovalTarget { app: "Notes", window_title: "Shopping\n list", element: None, hit: Hit::NotApplicable }), "Bring \"Shopping list\" in Notes to the front, switching desktops if needed");
    }
}

mod target_cases {
    use super::*;

    fn notes() -> DriverWindow {
        driver_window("w1", "Notes", 10, "Notes")
    }

    #[test]
    fn treats_numbers_from_an_earlier_run_as_unknown() {
        let mut earlier_run = ThreadTargets::starting_at(100);
        let listed = earlier_run.record_windows(&[notes()])[0].clone();
        let minted = earlier_run.record_observation(listed.id, &[element("e1", "button", "Save")])[0];

        // The next run mints from a different base; the agent still quotes the old ref.
        let mut this_run = ThreadTargets::starting_at(5_000);
        let relisted = this_run.record_windows(&[notes()])[0].clone();
        this_run.record_observation(relisted.id, &[element("e1", "button", "Save")]);

        assert_eq!(this_run.lookup_ref(minted), RefLookup::Unknown);
        assert_eq!(this_run.window(listed.id), None);
    }

    #[test]
    fn a_newer_observation_retires_the_windows_older_refs() {
        let mut targets = ThreadTargets::starting_at(100);
        let listed = targets.record_windows(&[notes(), driver_window("w2", "Mail", 11, "Inbox")]);
        let first = targets.record_observation(listed[0].id, &[element("e1", "button", "Save"), element("e2", "button", "Close")]);
        let other = targets.record_observation(listed[1].id, &[element("e3", "button", "Reply")]);
        let second = targets.record_observation(listed[0].id, &[element("e1", "button", "Save")]);
        assert_eq!(first, [100, 101]);
        assert_eq!(targets.lookup_ref(first[0]), RefLookup::Retired(RefRetired::NewerObservation));
        assert_eq!(targets.lookup_ref(first[1]), RefLookup::Retired(RefRetired::NewerObservation));
        // The record keeps what the agent was shown, for the live re-read at dispatch.
        let RefLookup::Found { reference, window } = targets.lookup_ref(second[0]) else { panic!("newest ref") };
        assert_eq!((reference.handle.as_str(), reference.identity(), window.id), ("e1", identity("button", "Save"), listed[0].id));
        // Observing one window leaves another window's refs alone.
        assert!(matches!(targets.lookup_ref(other[0]), RefLookup::Found { .. }));
        assert_eq!(targets.lookup_ref(second[0] + 1), RefLookup::Unknown);
        assert_eq!(targets.lookup_ref(99), RefLookup::Unknown);
    }

    #[test]
    fn only_a_windows_newest_shot_accepts_coordinates() {
        let mut targets = ThreadTargets::starting_at(100);
        let window = targets.record_windows(&[notes()])[0].clone();
        let shot = || NewShot { handle: "w1".into(), width: 400, height: 300, bounds: Rect { x: 0.0, y: 0.0, width: 400.0, height: 300.0 }, max_size: 1568 };
        let first = targets.record_shot(window.id, shot());
        let second = targets.record_shot(window.id, shot());
        assert_eq!(targets.lookup_shot(first.shot), ShotLookup::Retired(ShotRetired::NewerShot));
        assert!(matches!(targets.lookup_shot(second.shot), ShotLookup::Found { .. }));
        assert_eq!(targets.latest_shot(window.id).map(|shot| shot.shot), Some(second.shot));
        assert_eq!(targets.lookup_shot(second.shot + 1), ShotLookup::Unknown);
        // The window moved: its newest shot stops accepting coordinates too.
        targets.retire_shot(second.shot);
        assert_eq!(targets.lookup_shot(second.shot), ShotLookup::Retired(ShotRetired::NewerShot));
        assert_eq!(targets.latest_shot(window.id), None);
    }

    #[test]
    fn a_window_missing_from_a_listing_takes_its_refs_and_shot_with_it() {
        let mut targets = ThreadTargets::starting_at(100);
        let window = targets.record_windows(&[notes()])[0].clone();
        let reference = targets.record_observation(window.id, &[element("e1", "button", "Save")])[0];
        let shot = targets.record_shot(window.id, NewShot { handle: "w1".into(), width: 4, height: 3, bounds: Rect { x: 0.0, y: 0.0, width: 4.0, height: 3.0 }, max_size: 1568 });
        targets.record_windows(&[]);
        assert_eq!(targets.window(window.id), None);
        assert_eq!(targets.lookup_ref(reference), RefLookup::Retired(RefRetired::WindowClosed));
        // The shot went with the window, so it reads as retired rather than unknown.
        assert_eq!(targets.lookup_shot(shot.shot), ShotLookup::Retired(ShotRetired::NewerShot));
        // The same handle listed again is a new window: ids are never reused.
        let again = targets.record_windows(&[notes()])[0].clone();
        assert_eq!(again.id, window.id + 1);
    }

    #[test]
    fn a_handle_keeps_its_id_only_while_it_names_the_same_process_and_app() {
        let mut targets = ThreadTargets::starting_at(100);
        let first = targets.record_windows(&[notes()])[0].clone();
        let renamed = targets.record_windows(&[driver_window("w1", "Notes", 10, "Shopping")])[0].clone();
        assert_eq!((renamed.id, renamed.title.as_str()), (first.id, "Shopping"));
        let reference = targets.record_observation(first.id, &[element("e1", "button", "Save")])[0];
        // The handle now names another process: a different target under a new id.
        let reused = targets.record_windows(&[driver_window("w1", "Notes", 11, "Shopping")])[0].clone();
        assert_ne!(reused.id, first.id);
        assert_eq!(targets.window(first.id), None);
        assert_eq!(targets.lookup_ref(reference), RefLookup::Retired(RefRetired::WindowClosed));
        assert_eq!(targets.window(reused.id).map(|window| window.pid), Some(11));
    }

    #[test]
    fn reset_forgets_targets_but_never_reuses_a_number() {
        let mut targets = ThreadTargets::starting_at(100);
        let window = targets.record_windows(&[notes()])[0].clone();
        let reference = targets.record_observation(window.id, &[element("e1", "button", "Save")])[0];
        targets.reset();
        assert_eq!(targets.window(window.id), None);
        assert_eq!(targets.lookup_ref(reference), RefLookup::Retired(RefRetired::NewerObservation));
        let again = targets.record_windows(&[notes()])[0].clone();
        assert_eq!(again.id, window.id + 1);
        assert_eq!(targets.record_observation(again.id, &[element("e1", "button", "Save")]), [reference + 1]);
    }

    #[test]
    fn the_oldest_observations_go_first_once_a_thread_holds_too_many_refs() {
        let mut targets = ThreadTargets::starting_at(1);
        let listed = targets.record_windows(&(0..3).map(|index| driver_window(&format!("w{index}"), "Notes", 10, "n")).collect::<Vec<_>>());
        let many: Vec<DriverElement> = (0..900).map(|index| element(&format!("e{index}"), "button", "b")).collect();
        let first = targets.record_observation(listed[0].id, &many);
        let second = targets.record_observation(listed[1].id, &many);
        let third = targets.record_observation(listed[2].id, &many);
        assert_eq!(targets.lookup_ref(first[0]), RefLookup::Retired(RefRetired::NewerObservation));
        assert!(matches!(targets.lookup_ref(second[0]), RefLookup::Found { .. }));
        assert!(matches!(targets.lookup_ref(third[899]), RefLookup::Found { .. }));
    }

    #[test]
    fn a_run_starts_its_numbers_at_a_random_base() {
        let mut targets = ThreadTargets::default();
        let id = targets.record_windows(&[notes()])[0].id;
        assert!((100..=9_999_900).contains(&id) && id % 100 == 0, "{id}");
        assert_eq!(ThreadTargets::default().record_windows(&[notes()])[0].id, id, "one base for the whole run");
    }
}

mod command_cases {
    use super::*;

    /// This session's launcher; the only command word that is approved.
    const CLI: &str = "/home/u/.t3/userdata/computer-use/bin/viewcode-computer";
    /// A launcher path that needs quoting, as on macOS.
    const SPACED: &str = "/Users/Jane Doe/Library/Application Support/ViewCode/computer-use/bin/viewcode-computer";

    fn plain(script: &str, cli: &str) -> bool {
        is_plain_computer_use_command(Command::Script(script), cli)
    }

    fn plain_argv(argv: &[&str], cli: &str) -> bool {
        let argv: Vec<String> = argv.iter().map(|part| (*part).to_owned()).collect();
        is_plain_computer_use_command(Command::Argv(&argv), cli)
    }

    #[test]
    fn accepts_plain_invocations() {
        let accepted = [
            CLI.to_owned(),
            format!("{CLI} list-windows"),
            format!("  {CLI}\tlist-windows  "),
            format!("'{CLI}' list-windows"),
            format!("\"{CLI}\" list-windows"),
            format!("{CLI} observe --window 3 --query 'Save'"),
            format!("{CLI} type --ref 12 --text 'a;b'"),
            format!("{CLI} type --ref 12 --text 'it''s'"),
            format!("{CLI} type --ref 12 --text \"hello world\""),
            format!("{CLI} type --ref 12 --text \"it's fine\""),
            format!("{CLI} type --ref 12 --text 'héllo — 日本語 🙂'"),
            format!("{CLI} type --ref 12 --text 'a $(b) `c` \\d | e > f'"),
            format!("{CLI} type-focused --window 2 -"),
            format!("{CLI} key --window 2 --keys cmd+shift+t"),
            format!("{CLI} click --shot 4 --x=10 --y=20"),
        ];
        assert_eq!(accepted.len(), 15);
        for command in accepted {
            assert!(plain(&command, CLI), "{command:?}");
        }
    }

    #[test]
    fn rejects_everything_a_shell_would_interpret() {
        let rejected = [
            String::new(),
            "   ".to_owned(),
            // Only the exact launcher path: a bare name resolves through PATH.
            "viewcode-computer list-windows".to_owned(),
            "viewcode-computer".to_owned(),
            "'viewcode-computer' list-windows".to_owned(),
            format!("{CLI}x list-windows"),
            "./viewcode-computer list-windows".to_owned(),
            "/usr/local/bin/viewcode-computer list-windows".to_owned(),
            "/home/u/project/bin/viewcode-computer list-windows".to_owned(),
            format!("/tmp{CLI} list-windows"),
            format!("{CLI}/ list-windows"),
            format!("'{CLI}'' list-windows"),
            format!("{CLI}'' list-windows"),
            format!("'{CLI}'x list-windows"),
            "'/home/u/.t3/userdata/computer-use/bin/'viewcode-computer list-windows".to_owned(),
            format!("X=1 {CLI} list-windows"),
            format!("sudo {CLI} list-windows"),
            format!("env {CLI} list-windows"),
            format!("command {CLI} list-windows"),
            format!("{CLI} list-windows; rm -rf ~"),
            format!("{CLI} list-windows && curl evil.example"),
            format!("{CLI} list-windows || true"),
            format!("{CLI} list-windows | tee out"),
            format!("{CLI} list-windows &"),
            format!("{CLI} list-windows > out.txt"),
            format!("{CLI} type-focused --window 2 - < ~/.ssh/id_rsa"),
            format!("{CLI} type-focused --window 2 - <<EOF"),
            format!("{CLI} type --ref 1 --text \"$(cat ~/.ssh/id_rsa)\""),
            format!("{CLI} type --ref 1 --text $(cat ~/.ssh/id_rsa)"),
            format!("{CLI} type --ref 1 --text `cat ~/.ssh/id_rsa`"),
            format!("{CLI} type --ref 1 --text \"`id`\""),
            format!("{CLI} type --ref 1 --text $HOME"),
            format!("{CLI} type --ref 1 --text \"${{HOME}}\""),
            format!("{CLI} type --ref 1 --text \"a\\\"b\""),
            format!("{CLI} type --ref 1 --text a\\;b"),
            format!("{CLI} list-windows\n"),
            format!("{CLI} list-windows\nrm -rf ~"),
            format!("{CLI} list-windows\r"),
            format!("{CLI} list-windows rm -rf ~"),
            format!("{CLI} list-windows\u{0}"),
            format!("{CLI} type --ref 1 --text 'unterminated"),
            format!("{CLI} type --ref 1 --text \"unterminated"),
            format!("{CLI} screenshot --window * "),
            format!("{CLI} screenshot --window ?"),
            format!("{CLI} screenshot --window [1]"),
            format!("{CLI} screenshot --window {{1,2}}"),
            format!("{CLI} screenshot --window ~"),
            format!("{CLI} list-windows # comment"),
            format!("{CLI} list-windows (x)"),
            format!("{CLI} type --ref 1 --text \"hi!\""),
            format!("bash -lc '{CLI} list-windows; id'"),
            format!("bash -lc '{CLI} list-windows' extra"),
            "bash -lc 'viewcode-computer list-windows'".to_owned(),
            format!("python -c '{CLI} list-windows'"),
        ];
        assert_eq!(rejected.len(), 54);
        for command in rejected {
            assert!(!plain(&command, CLI), "{command:?}");
        }
    }

    #[test]
    fn accepts_a_shell_joined_wrapping_shell_around_a_plain_invocation() {
        assert!(plain(&format!("bash -lc '{CLI} list-windows'"), CLI));
        assert!(plain(&format!("/bin/zsh -c '{CLI} observe --window 1'"), CLI));
        // shlex quoting of an embedded single quote
        assert!(plain(&format!("/bin/bash -lc '{CLI} type --ref 1 --text '\"'\"'a;b'\"'\"''"), CLI));
    }

    #[test]
    fn accepts_the_exact_launcher_path_quoted_when_it_contains_spaces() {
        assert!(plain(&format!("'{SPACED}' list-windows"), SPACED));
        assert!(plain(&format!("\"{SPACED}\" status"), SPACED));
        assert!(plain_argv(&[SPACED, "status"], SPACED));
        assert!(plain_argv(&["bash", "-lc", &format!("'{SPACED}' status")], SPACED));
        // Codex reports the argv shell-joined, quoting the script once more.
        assert!(plain(&format!("/bin/bash -lc ''\"'\"'{SPACED}'\"'\"' status'"), SPACED));
        assert!(!plain(&format!("{SPACED} list-windows"), SPACED));
        assert!(!plain("'/Users/Jane Doe'/Library/x list-windows", SPACED));
        assert!(!plain(&format!("'{SPACED}' status"), CLI));
    }

    #[test]
    fn never_approves_a_relative_or_malformed_launcher_path() {
        assert!(!plain("viewcode-computer status", "viewcode-computer"));
        assert!(!plain_argv(&["bin/viewcode-computer"], "bin/viewcode-computer"));
        assert!(!plain(&format!("{CLI} status"), &format!("{CLI}\n")));
    }

    #[test]
    fn argv_is_executed_as_is_unless_it_wraps_a_script() {
        let list = format!("{CLI} list-windows");
        let chained = format!("{CLI} list-windows; id");
        let longer = format!("{CLI}x");
        let (list, chained, longer) = (list.as_str(), chained.as_str(), longer.as_str());
        let cases: [(&[&str], bool); 21] = [
            (&[CLI, "list-windows"], true),
            (&[CLI, "type", "--ref", "1", "--text", "a; rm -rf ~ $(id)"], true),
            (&["bash", "-lc", list], true),
            (&["/bin/sh", "-c", list], true),
            (&["/usr/bin/zsh", "-lc", list], true),
            (&[], false),
            (&["viewcode-computer", "list-windows"], false),
            (&["bash", "-lc", "viewcode-computer list-windows"], false),
            (&[longer, "list-windows"], false),
            (&["./viewcode-computer", "list-windows"], false),
            (&["/tmp/viewcode-computer", "list-windows"], false),
            (&["env", CLI, "list-windows"], false),
            (&[CLI, "type", "--text", "a\nb"], false),
            (&[CLI, "type", "--text", "a\u{0}b"], false),
            (&["bash", "-lc", chained], false),
            (&["bash", "-lc", list, "extra"], false),
            (&["bash", "-x", list], false),
            (&["bash", CLI], false),
            (&["./bash", "-c", list], false),
            (&["/tmp/bash", "-c", list], false),
            (&["fish", "-c", list], false),
        ];
        for (argv, expected) in cases {
            assert_eq!(plain_argv(argv, CLI), expected, "{argv:?}");
        }
    }

    fn session(cli: &str, shots: Option<&str>) -> SessionGrant {
        SessionGrant { cli_path: cli.to_owned(), screenshots_dir: shots.map(str::to_owned) }
    }

    #[test]
    fn requires_a_session_with_computer_use_and_its_own_absolute_launcher() {
        let list = json!(format!("{CLI} list-windows"));
        // No session, and a session spawned without computer use (it holds no grant).
        assert!(!auto_approves_computer_use_command(None, &list));
        let config = session(CLI, None);
        assert!(auto_approves_computer_use_command(Some(&config), &list));
        assert!(auto_approves_computer_use_command(Some(&config), &json!([CLI, "status"])));
        assert!(!auto_approves_computer_use_command(Some(&config), &json!("viewcode-computer list-windows")));
        assert!(!auto_approves_computer_use_command(Some(&config), &json!(format!("{CLI} list-windows; id"))));
        assert!(!auto_approves_computer_use_command(Some(&config), &json!([CLI, 1])));
        assert!(!auto_approves_computer_use_command(Some(&config), &Value::Null));
        let spaced = session(SPACED, None);
        assert!(auto_approves_computer_use_command(Some(&spaced), &json!(format!("'{SPACED}' list-windows"))));
        assert!(!auto_approves_computer_use_command(Some(&spaced), &list));
        assert!(!auto_approves_computer_use_command(Some(&session("viewcode-computer", None)), &json!("viewcode-computer list-windows")));
    }

    struct Shots {
        root: std::path::PathBuf,
        config: SessionGrant,
        shots: String,
        shot: String,
        secret: String,
    }

    impl Drop for Shots {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn setup() -> Shots {
        let root = std::env::temp_dir().join(format!("pandacode-cu-shots-{}", uuid::Uuid::new_v4()));
        let shots = root.join("screenshots");
        std::fs::create_dir_all(&shots).unwrap();
        let shot = shots.join("shot-1.png");
        std::fs::write(&shot, "png").unwrap();
        let secret = root.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, shots.join("planted.png")).unwrap();
        let text = |path: &std::path::Path| path.to_string_lossy().into_owned();
        Shots { config: session(CLI, Some(&text(&shots))), shots: text(&shots), shot: text(&shot), secret: text(&secret), root }
    }

    #[test]
    fn allows_only_existing_files_inside_the_sessions_own_screenshot_directory() {
        let fixture = setup();
        let config = Some(&fixture.config);
        assert!(is_computer_use_screenshot_read(config, &json!(fixture.shot)));
        for path in [fixture.shots.clone(), fixture.secret.clone(), format!("{}/../secret.txt", fixture.shots), format!("{}/planted.png", fixture.shots), format!("{}/missing.png", fixture.shots), "screenshots/shot-1.png".to_owned()] {
            assert!(!is_computer_use_screenshot_read(config, &json!(path)), "{path}");
        }
        assert!(!is_computer_use_screenshot_read(None, &json!(fixture.shot)));
        assert!(!is_computer_use_screenshot_read(Some(&session(CLI, None)), &json!(fixture.shot)), "a session without a screenshot folder");
    }

    #[test]
    fn answers_an_acp_screenshot_read_only_when_every_supplied_path_agrees() {
        let fixture = setup();
        let config = Some(&fixture.config);
        let (shot, secret) = (&fixture.shot, &fixture.secret);
        let request = |tool_call: Value| json!({ "toolCall": tool_call, "options": [{ "optionId": "allow", "kind": "allow_once" }, { "optionId": "reject", "kind": "reject_once" }] });
        let answer = |tool_call: Value| acp_computer_use_allow_once_option(config, &request(tool_call), AcpPermissionRules::default()).map(str::to_owned);
        assert_eq!(answer(json!({ "kind": "read", "locations": [{ "path": shot }], "rawInput": { "path": shot } })).as_deref(), Some("allow"));
        assert_eq!(answer(json!({ "kind": "read", "rawInput": { "path": shot } })).as_deref(), Some("allow"));
        assert_eq!(answer(json!({ "kind": "read", "locations": [{ "path": shot }], "rawInput": { "path": secret } })), None);
        assert_eq!(answer(json!({ "kind": "read", "rawInput": {} })), None);
        assert_eq!(answer(json!({ "kind": "edit", "locations": [{ "path": shot }], "rawInput": { "path": shot } })), None);
    }

    #[test]
    fn answers_an_acp_shell_request_only_for_the_plain_launcher() {
        let config = session(CLI, None);
        let request = |tool_call: Value| json!({ "toolCall": tool_call, "options": [{ "optionId": " ", "kind": "allow_once" }, { "optionId": "once", "kind": "allow_once" }, { "optionId": "always", "kind": "allow_always" }] });
        let answer = |tool_call: Value, rules: AcpPermissionRules<'_>| acp_computer_use_allow_once_option(Some(&config), &request(tool_call), rules).map(str::to_owned);
        let list = format!("{CLI} list-windows");
        assert_eq!(answer(json!({ "kind": "execute", "rawInput": { "command": list } }), AcpPermissionRules::default()).as_deref(), Some("once"));
        assert_eq!(answer(json!({ "kind": "execute", "rawInput": { "command": format!("{list}; id") } }), AcpPermissionRules::default()), None);
        assert_eq!(answer(json!({ "kind": "execute", "rawInput": {} }), AcpPermissionRules::default()), None);
        assert_eq!(answer(json!({ "kind": "execute", "rawInput": list }), AcpPermissionRules::default()), None);
        // The display title is never read, and two command fields cannot disagree.
        assert_eq!(answer(json!({ "kind": "execute", "title": list, "rawInput": { "command": "rm -rf ~" } }), AcpPermissionRules::default()), None);
        let both = AcpPermissionRules { command_fields: &["command", "CommandLine"], allow_missing_kind: true };
        assert_eq!(answer(json!({ "rawInput": { "command": list, "CommandLine": "id" } }), both), None);
        assert_eq!(answer(json!({ "kind": null, "rawInput": { "CommandLine": list } }), both).as_deref(), Some("once"));
        assert_eq!(answer(json!({ "rawInput": { "command": list } }), AcpPermissionRules::default()), None, "a missing kind only where the adapter says so");
        assert_eq!(acp_computer_use_allow_once_option(None, &request(json!({ "kind": "execute", "rawInput": { "command": list } })), AcpPermissionRules::default()), None);
    }
}

mod gate {
    use super::*;

    const NOTES: &str = "wab12.1";

    fn own() -> HashSet<u32> {
        HashSet::from([4242])
    }

    fn turn(id: &str, full_access: bool) -> TurnContext {
        TurnContext { title: "Tidy notes".into(), turn_id: id.into(), full_access }
    }

    fn live<'a>(turn: Option<&'a TurnContext>, own_pids: &'a HashSet<u32>) -> Live<'a> {
        Live { mode: Mode::Control, approvals: Approvals::Thread, screen: Screen::Allow, turn, approval_pending: false, own_pids, background_input: false }
    }

    /// A thread that listed Notes (`100`) and 1Password (`101`), observed Notes (refs
    /// `100` Done, `101` Delete) and took a screenshot of it (shot `100`).
    fn observed() -> ThreadGate {
        let mut gate = ThreadGate::starting_at(100);
        let mut vault = driver_window("wab12.2", "1Password", 71, "Vault");
        vault.app_identifier = Some("/Applications/1Password.app/Contents/MacOS/1Password".into());
        let listed = gate.targets.record_windows(&[driver_window(NOTES, "Notes", 70, "Shopping"), vault]);
        gate.targets.record_observation(listed[0].id, &[element("eab12.3", "button", "Done"), element("eab12.4", "button", "Delete")]);
        gate.targets.record_shot(listed[0].id, NewShot { handle: NOTES.into(), width: 400, height: 300, bounds: Rect { x: 100.0, y: 50.0, width: 800.0, height: 600.0 }, max_size: 1568 });
        gate
    }

    fn press(reference: u64) -> InputRequest {
        InputRequest::Press { reference }
    }

    fn refusal<T: std::fmt::Debug>(result: Result<T, ComputerUseError>) -> (ErrorCode, String, Option<Effect>) {
        let error = result.expect_err("a refusal");
        (error.code, error.message, error.effect)
    }

    fn native(handle: &str, element: Option<ElementIdentity>) -> DispatchTarget {
        DispatchTarget { window: driver_window(handle, "Notes", 70, "Shopping"), element, point: None, foreground: false }
    }

    #[test]
    fn the_mode_decides_what_a_request_may_do_at_all() {
        let press = json!({ "command": "press", "ref": 100 });
        assert_eq!(refusal(admit(Mode::Off, &press)), (ErrorCode::Off, "Computer use is turned off in PandaCode settings.".into(), None));
        // Off refuses before the request is even read.
        assert_eq!(refusal(admit(Mode::Off, &json!(7))).0, ErrorCode::Off);
        assert_eq!(refusal(admit(Mode::Observe, &press)), (ErrorCode::ObserveOnly, "Computer use is set to observe only; input actions are unavailable. Ask the user to switch it to control in PandaCode settings.".into(), Some(Effect::NotDispatched)));
        for look in [json!({ "command": "status" }), json!({ "command": "list-windows", "app": "notes" }), json!({ "command": "observe", "window": 100 }), json!({ "command": "screenshot", "window": 100, "maxSize": 1024 })] {
            assert!(admit(Mode::Observe, &look).is_ok(), "{look}");
        }
        assert_eq!(admit(Mode::Control, &press), Ok(Request::Input(InputRequest::Press { reference: 100 })));
        // An unreadable setting must never turn computer use on, and falls back to the strictest answers.
        assert_eq!((Mode::default(), Approvals::default(), Screen::UNREADABLE), (Mode::Off, Approvals::Thread, Screen::Ask));
        assert_eq!(serde_json::to_value([Mode::Off, Mode::Observe, Mode::Control]).unwrap(), json!(["off", "observe", "control"]));
        assert_eq!(serde_json::to_value([Approvals::Thread, Approvals::Risky, Approvals::Never]).unwrap(), json!(["thread", "risky", "never"]));
    }

    #[test]
    fn validates_untrusted_requests_without_echoing_their_values() {
        let message = |body: Value| refusal(validate_request(&body));
        assert_eq!(message(json!([1])), (ErrorCode::UnknownCommand, "The request must be a JSON object with a command.".into(), None));
        assert_eq!(message(json!({ "command": "teleport" })), (ErrorCode::UnknownCommand, "Unknown command. Run `viewcode-computer help` for the list of commands.".into(), None));
        assert_eq!(message(json!({ "command": "press", "ref": 1, "secret-argument": "hunter2" })), (ErrorCode::UnexpectedArgument, "`press` does not accept: secret-argument.".into(), None));
        assert_eq!(message(json!({ "command": "status", ("a".repeat(60)): 1 })).1, format!("`status` does not accept: {}….", "a".repeat(40)));
        assert_eq!(message(json!({ "command": "scroll", "ref": 1 })), (ErrorCode::MissingArgument, "`scroll` requires: dx, dy.".into(), None));
        assert_eq!(message(json!({ "command": "type", "ref": 0, "text": "hunter2" })), (ErrorCode::InvalidArgument, "`type` has an invalid ref. Run `viewcode-computer help` for the accepted values.".into(), None));
        for invalid in [
            json!({ "command": "type", "ref": 1, "text": "" }),
            json!({ "command": "type", "ref": 1, "text": "x".repeat(10_001) }),
            json!({ "command": "key", "window": 1, "keys": "cmd+" }),
            json!({ "command": "key", "window": 1, "keys": "CMD+S" }),
            json!({ "command": "key", "window": 1, "keys": "f25" }),
            json!({ "command": "scroll", "ref": 1, "dx": 0, "dy": 51 }),
            json!({ "command": "click", "shot": 1, "x": -1, "y": 0 }),
            json!({ "command": "click", "shot": 1, "x": 1.5, "y": 0 }),
            json!({ "command": "click", "shot": 1, "x": 1, "y": 0, "count": 4 }),
            json!({ "command": "click", "shot": 1, "x": 1, "y": 0, "button": "back" }),
            json!({ "command": "click", "shot": 1, "x": 1, "y": 0, "button": null }),
            json!({ "command": "screenshot", "window": 1, "maxSize": 255 }),
            json!({ "command": "observe", "window": "1" }),
            json!({ "command": "list-windows", "app": "a".repeat(201) }),
        ] {
            let (code, text, _) = message(invalid.clone());
            assert_eq!(code, ErrorCode::InvalidArgument, "{invalid}");
            assert!(!text.contains("hunter2") && !text.contains("xxx"), "{text}");
        }
        assert_eq!(validate_request(&json!({ "command": "click", "shot": 3.0, "x": 10, "y": 20, "button": "right", "count": 2 })), Ok(Request::Input(InputRequest::Click { shot: 3, x: 10, y: 20, button: Some(MouseButton::Right), count: Some(2) })));
        assert_eq!(validate_request(&json!({ "command": "key", "window": 2, "keys": "cmd+shift+f12" })), Ok(Request::Input(InputRequest::Key { window: 2, keys: "cmd+shift+f12".into() })));
        assert_eq!(validate_request(&json!({ "command": "scroll-at", "shot": 3, "x": 1, "y": 2, "dx": -50, "dy": 50 })), Ok(Request::Input(InputRequest::ScrollAt { shot: 3, x: 1, y: 2, dx: -50, dy: 50 })));
        assert_eq!(validate_request(&json!({ "command": "drag", "shot": 3, "fromX": 1, "fromY": 2, "toX": 3, "toY": 4 })), Ok(Request::Input(InputRequest::Drag { shot: 3, from_x: 1, from_y: 2, to_x: 3, to_y: 4 })));
        // Every command name round-trips.
        for (body, command) in [(json!({ "command": "set-value", "ref": 1, "value": "v" }), "set-value"), (json!({ "command": "type-focused", "window": 1, "text": "t" }), "type-focused"), (json!({ "command": "focus", "window": 1 }), "focus"), (json!({ "command": "move", "shot": 1, "x": 0, "y": 0 }), "move"), (json!({ "command": "list-windows" }), "list-windows")] {
            assert_eq!(validate_request(&body).unwrap().command(), command);
        }
    }

    #[test]
    fn protected_apps_are_never_observed_or_controlled() {
        let gate = observed();
        let own = own();
        let denied = "This app can never be observed or controlled through computer use (password managers, system settings and PandaCode itself). Ask the user to do this step.";
        assert_eq!(refusal(observable_window(&gate.targets, 101, &own)), (ErrorCode::ProtectedApp, denied.into(), None));
        assert_eq!(refusal(observable_window(&gate.targets, 555, &own)), (ErrorCode::UnknownWindow, "Window 555 is not known to this thread (never listed, or closed). Run `viewcode-computer list-windows`.".into(), None));
        assert_eq!(observable_window(&gate.targets, 100, &own).map(|window| window.handle), Ok(NOTES.into()));
        let key = InputRequest::Key { window: 101, keys: "enter".into() };
        assert_eq!(refusal(resolve_input_target(&gate.targets, &key, &own)), (ErrorCode::ProtectedApp, denied.into(), Some(Effect::NotDispatched)));
        // A window of the app's own process, or of the process that started it, whatever it is called.
        let ancestor = HashSet::from([70]);
        assert_eq!(refusal(observable_window(&gate.targets, 100, &ancestor)).0, ErrorCode::ProtectedApp);
        assert_eq!(refusal(resolve_input_target(&gate.targets, &press(100), &ancestor)).0, ErrorCode::ProtectedApp);
        assert_eq!(refusal(resolve_input_target(&gate.targets, &InputRequest::Move { shot: 100, x: 1, y: 1 }, &ancestor)).0, ErrorCode::ProtectedApp);
        assert!(own_process_floor().contains(&std::process::id()));
        assert!(!is_denied(app("Notes", None), &HashSet::new()) && is_denied(AppWindow { app: "Notes", app_identifier: None, title: None, pid: Some(std::process::id()) }, &own_process_floor()));
    }

    #[test]
    fn screenshots_are_refused_while_a_protected_app_overlaps_the_window() {
        let rect = |x: f64, y: f64| Some(Rect { x, y, width: 100.0, height: 100.0 });
        let window = |handle: &str, app: &str, bounds: Option<Rect>| DriverWindow { bounds, ..driver_window(handle, app, 70, "t") };
        let own = own();
        let notes = window("w1", "Notes", rect(0.0, 0.0));
        let overlaps = |listed: &[DriverWindow]| protected_overlay(listed, "w1", &own);
        assert!(!overlaps(&[notes.clone(), window("w2", "Mail", rect(50.0, 50.0))]));
        assert!(overlaps(&[notes.clone(), window("w2", "1Password", rect(50.0, 50.0))]));
        assert!(overlaps(&[notes.clone(), window("w2", "System Settings", rect(99.0, 99.0))]));
        // Touching edges share no area.
        assert!(!overlaps(&[notes.clone(), window("w2", "Keychain Access", rect(100.0, 0.0))]));
        // Unknown bounds count as overlapping, on either side.
        assert!(overlaps(&[notes.clone(), window("w2", "Bitwarden", None)]));
        assert!(overlaps(&[window("w1", "Notes", None), window("w2", "Bitwarden", rect(900.0, 900.0))]));
        assert!(overlaps(&[window("w2", "Bitwarden", rect(900.0, 900.0))]), "the target is no longer listed");
        // The app's own window refuses input but does not block screenshots of what it overlaps.
        assert!(!overlaps(&[notes.clone(), window("w2", "PandaCode", rect(50.0, 50.0))]));
        // The target itself became a protected app, or is the app's own process.
        assert!(overlaps(&[window("w1", "1Password", rect(0.0, 0.0))]));
        assert!(protected_overlay(&[notes], "w1", &HashSet::from([70])));
        assert_eq!(overlaid().message, "A protected window overlaps this window; move it away and try again.");
        assert!(rects_intersect(&rect(0.0, 0.0).unwrap(), &rect(99.9, 99.9).unwrap()));
    }

    #[test]
    fn stale_and_unknown_targets_are_refused_with_what_to_do_next() {
        let mut gate = observed();
        let own = own();
        let resolve = |gate: &ThreadGate, request: &InputRequest| refusal(resolve_input_target(&gate.targets, request, &own));
        assert_eq!(resolve(&gate, &press(999)), (ErrorCode::UnknownRef, "Ref 999 is not known to this thread. Observe the window to get refs.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Move { shot: 999, x: 1, y: 1 }), (ErrorCode::UnknownShot, "Shot 999 is not known to this thread. Take a screenshot first.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Focus { window: 999 }), (ErrorCode::UnknownWindow, "Window 999 is not known to this thread (never listed, or closed). Run `viewcode-computer list-windows`.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Click { shot: 100, x: 400, y: 0, button: None, count: None }), (ErrorCode::InvalidArgument, "A point is outside shot 100, which is 400×300 pixels.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Drag { shot: 100, from_x: 0, from_y: 0, to_x: 399, to_y: 300 }).0, ErrorCode::InvalidArgument);
        assert!(resolve_input_target(&gate.targets, &InputRequest::Click { shot: 100, x: 399, y: 299, button: None, count: None }, &own).is_ok());

        // A newer observation and a newer screenshot retire the older numbers.
        gate.targets.record_observation(100, &[element("eab12.5", "button", "Done")]);
        gate.targets.record_shot(100, NewShot { handle: NOTES.into(), width: 400, height: 300, bounds: Rect { x: 100.0, y: 50.0, width: 800.0, height: 600.0 }, max_size: 1568 });
        assert_eq!(resolve(&gate, &press(100)), (ErrorCode::StaleRef, "Ref 100 is from an older observation; observe again.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Move { shot: 100, x: 1, y: 1 }), (ErrorCode::StaleShot, "Shot 100 is not the newest screenshot of its window; use the newest shot (every input returns one) or take a new screenshot.".into(), Some(Effect::NotDispatched)));
        assert!(matches!(resolve_input_target(&gate.targets, &press(102), &own), Ok(ResolvedTarget::Element { .. })));
        assert!(matches!(resolve_input_target(&gate.targets, &InputRequest::Move { shot: 101, x: 1, y: 1 }, &own), Ok(ResolvedTarget::Shot { .. })));

        // The window closed.
        gate.targets.record_windows(&[]);
        assert_eq!(resolve(&gate, &press(102)), (ErrorCode::StaleRef, "Ref 102 belongs to a window that closed. Run `viewcode-computer list-windows`.".into(), Some(Effect::NotDispatched)));
        assert_eq!(resolve(&gate, &InputRequest::Move { shot: 101, x: 1, y: 1 }).0, ErrorCode::StaleShot);
    }

    #[test]
    fn input_needs_a_running_turn_and_no_waiting_approval() {
        let mut gate = observed();
        let own = own();
        assert_eq!(refusal(begin_input(&live(None, &own), &gate, &press(100))), (ErrorCode::NoTurn, "No turn is running in this thread; computer actions only happen while you are working on the user's request.".into(), Some(Effect::NotDispatched)));
        let running = turn("turn-1", true);
        let paused = Live { approval_pending: true, ..live(Some(&running), &own) };
        assert_eq!(refusal(begin_input(&paused, &gate, &press(100))), (ErrorCode::ApprovalPending, "Input is paused while an approval waits for the user; wait for it to be answered, then observe again.".into(), Some(Effect::NotDispatched)));
        // The pause applies in Full access and with approvals set to never.
        let never = Live { approvals: Approvals::Never, ..paused };
        assert_eq!(refusal(begin_input(&never, &gate, &press(100))).0, ErrorCode::ApprovalPending);
        assert!(begin_input(&live(Some(&running), &own), &gate, &press(100)).is_ok());

        // A turn this gate was told ended is not running, whatever storage still says.
        assert_eq!(gate.running_turn(Some(running.clone())), Some(running.clone()));
        gate.end_turn(None, None);
        assert_eq!(gate.running_turn(Some(running)), None);
        assert_eq!(gate.running_turn(Some(turn("turn-2", false))).map(|turn| turn.turn_id), Some("turn-2".into()));
    }

    #[test]
    fn follow_thread_permissions_asks_unless_full_access_or_allowed_for_the_turn() {
        let mut gate = observed();
        let own = own();
        let sandboxed = turn("turn-1", false);
        let (resolved, step) = begin_input(&live(Some(&sandboxed), &own), &gate, &press(100)).unwrap();
        assert_eq!(step, ScreenStep::Proceed);
        let decision = decide_input(Approvals::Thread, &gate, &sandboxed, &press(100), &resolved, &Hit::NotApplicable);
        let InputDecision::Ask { prompt, destructive: false } = &decision else { panic!("{decision:?}") };
        assert_eq!(prompt.detail, "Press button \"Done\" in Notes — \"Shopping\"");
        assert_eq!(prompt.options.iter().map(|option| option.label).collect::<Vec<_>>(), ["Allow once", "Allow for the rest of this turn", "Decline"]);
        assert_eq!(serde_json::to_value(prompt.options).unwrap(), json!([{ "decision": "accept", "label": "Allow once" }, { "decision": "acceptForSession", "label": "Allow for the rest of this turn" }, { "decision": "decline", "label": "Decline" }]));
        assert!(decision.asked() && !decision.destructive());

        // Full access: routine input runs without asking.
        let full = turn("turn-1", true);
        assert_eq!(decide_input(Approvals::Thread, &gate, &full, &press(100), &resolved, &Hit::NotApplicable), InputDecision::Allow { destructive: false });

        // "Allow once" covers that action only.
        assert_eq!(gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::Accept)), Ok(()));
        assert!(decide_input(Approvals::Thread, &gate, &sandboxed, &press(100), &resolved, &Hit::NotApplicable).asked());
        // "Allow for the rest of this turn" covers routine input until the turn ends.
        assert_eq!(gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::AcceptForSession)), Ok(()));
        assert_eq!(decide_input(Approvals::Thread, &gate, &sandboxed, &press(100), &resolved, &Hit::NotApplicable), InputDecision::Allow { destructive: false });
        assert!(decide_input(Approvals::Thread, &gate, &turn("turn-2", false), &press(100), &resolved, &Hit::NotApplicable).asked(), "the next turn asks again");
        gate.end_turn(Some("turn-0"), None);
        assert!(gate.granted(&sandboxed), "another turn ending leaves this turn's grant");
        gate.end_turn(Some("turn-1"), None);
        assert!(!gate.granted(&sandboxed));
        // An interrupt (no id) and a stopped session drop it too.
        for end in [|gate: &mut ThreadGate| gate.end_turn(None, Some("turn-1")), |gate: &mut ThreadGate| gate.release()] {
            assert_eq!(gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::AcceptAlways)), Ok(()));
            assert!(gate.granted(&sandboxed));
            end(&mut gate);
            assert!(!gate.granted(&sandboxed));
        }
    }

    #[test]
    fn a_declined_or_unanswered_approval_refuses_the_action() {
        let mut gate = observed();
        let declined = (ErrorCode::Declined, "The user declined this action, or no answer arrived.".to_owned(), Some(Effect::NotDispatched));
        assert_eq!(refusal(gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::Decline))), declined);
        assert_eq!(refusal(gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::Cancel))), declined);
        assert_eq!(refusal(gate.answer_approval("turn-1", false, AskOutcome::Paused)).0, ErrorCode::ApprovalPending);
        assert!(!gate.granted(&turn("turn-1", false)));
        assert_eq!(APPROVAL_TIMEOUT_SECS, 600);
    }

    #[test]
    fn destructive_looking_input_always_asks_and_is_never_covered_by_the_turn_grant() {
        let mut gate = observed();
        let own = own();
        let full = turn("turn-1", true);
        let live = live(Some(&full), &own);
        let (delete, _) = begin_input(&live, &gate, &press(101)).unwrap();
        let decision = decide_input(Approvals::Thread, &gate, &full, &press(101), &delete, &Hit::NotApplicable);
        let InputDecision::Ask { prompt, destructive: true } = &decision else { panic!("{decision:?}") };
        assert_eq!(prompt.detail, "Press button \"Delete\" in Notes — \"Shopping\"");
        assert_eq!(serde_json::to_value(prompt.options).unwrap(), json!([{ "decision": "accept", "label": "Allow once", "warning": "Looks destructive, so this always asks" }, { "decision": "decline", "label": "Decline" }]));
        // Accepting it "for the session" grants nothing.
        assert_eq!(gate.answer_approval("turn-1", true, AskOutcome::Answered(ApprovalAnswer::AcceptForSession)), Ok(()));
        assert!(!gate.granted(&turn("turn-1", false)));
        gate.answer_approval("turn-1", false, AskOutcome::Answered(ApprovalAnswer::AcceptForSession)).unwrap();
        assert!(decide_input(Approvals::Thread, &gate, &turn("turn-1", false), &press(101), &delete, &Hit::NotApplicable).asked());

        // Quit and close chords, and what a click lands on.
        let key = |keys: &str| InputRequest::Key { window: 100, keys: keys.into() };
        let (window, _) = begin_input(&live, &gate, &key("cmd+q")).unwrap();
        assert!(decide_input(Approvals::Thread, &gate, &full, &key("cmd+q"), &window, &Hit::NotApplicable).destructive());
        assert_eq!(decide_input(Approvals::Thread, &gate, &full, &key("cmd+s"), &window, &Hit::NotApplicable), InputDecision::Allow { destructive: false });
        let click = InputRequest::Click { shot: 100, x: 10, y: 10, button: None, count: None };
        let (shot, _) = begin_input(&live, &gate, &click).unwrap();
        let on_delete = Hit::Element(identity("button", "Delete note"));
        let decision = decide_input(Approvals::Thread, &gate, &full, &click, &shot, &on_delete);
        let InputDecision::Ask { prompt, destructive: true } = &decision else { panic!("{decision:?}") };
        assert_eq!(prompt.detail, "Click at (10, 10) in Notes — \"Shopping\" (on button \"Delete note\"); brings the window to the front");
        assert_eq!(decide_input(Approvals::Thread, &gate, &full, &click, &shot, &Hit::Nothing), InputDecision::Allow { destructive: false });
        // A move or scroll over a "Delete" button is not asked about.
        let hover = InputRequest::Move { shot: 100, x: 10, y: 10 };
        assert_eq!(decide_input(Approvals::Thread, &gate, &full, &hover, &shot, &on_delete), InputDecision::Allow { destructive: false });
        // What the hit test reads: the centre of the first pixel, through the shot's screen rect.
        assert_eq!(hit_point(&click, &shot).map(|point| (point.x, point.y)), Some((121.0, 71.0)));
        assert_eq!(hit_point(&press(101), &delete), None);
    }

    #[test]
    fn risky_only_and_never_settings() {
        let gate = observed();
        let own = own();
        let sandboxed = turn("turn-1", false);
        let live = live(Some(&sandboxed), &own);
        let (done, _) = begin_input(&live, &gate, &press(100)).unwrap();
        let (delete, _) = begin_input(&live, &gate, &press(101)).unwrap();
        // Risky: only destructive-looking input asks, in any permission mode.
        assert_eq!(decide_input(Approvals::Risky, &gate, &sandboxed, &press(100), &done, &Hit::NotApplicable), InputDecision::Allow { destructive: false });
        assert!(decide_input(Approvals::Risky, &gate, &sandboxed, &press(101), &delete, &Hit::NotApplicable).asked());
        assert!(decide_input(Approvals::Risky, &gate, &turn("turn-1", true), &press(101), &delete, &Hit::NotApplicable).asked());
        // Never: nothing asks, but the action is still known to look destructive.
        assert_eq!(decide_input(Approvals::Never, &gate, &sandboxed, &press(101), &delete, &Hit::NotApplicable), InputDecision::Allow { destructive: true });
        for (approvals, destructive, granted, asks) in [(Approvals::Thread, false, false, true), (Approvals::Thread, false, true, false), (Approvals::Thread, true, true, true), (Approvals::Risky, false, false, false), (Approvals::Risky, true, true, true), (Approvals::Never, true, false, false)] {
            assert_eq!(input_needs_approval(approvals, destructive, granted), asks, "{approvals:?} {destructive} {granted}");
        }
    }

    #[test]
    fn show_on_screen_is_asked_once_per_turn_before_input_that_takes_the_screen() {
        let mut gate = observed();
        let own = own();
        let full = turn("turn-1", true);
        let ask = Live { screen: Screen::Ask, ..live(Some(&full), &own) };
        let key = InputRequest::Key { window: 100, keys: "enter".into() };
        // Press, set-value and type by ref act in the background and never ask.
        for by_ref in [press(100), InputRequest::SetValue { reference: 100, value: "v".into() }, InputRequest::Type { reference: 100, text: "t".into() }] {
            assert_eq!(begin_input(&ask, &gate, &by_ref).unwrap().1, ScreenStep::Proceed);
        }
        let (_, step) = begin_input(&ask, &gate, &key).unwrap();
        let ScreenStep::Ask(prompt) = step else { panic!("asks first") };
        assert_eq!(prompt.detail, "Show this on your screen? To work in Notes, the agent needs to bring its windows to the front and use the mouse and keyboard until this response ends. In the background it can only act on controls directly.");
        assert_eq!(prompt.options.iter().map(|option| (option.decision, option.label)).collect::<Vec<_>>(), [(ApprovalAnswer::AcceptForSession, "Show on screen for this task"), (ApprovalAnswer::Decline, "Keep it in the background")]);
        // Scrolling a ref, coordinates, typing into the focus and focusing all take the screen.
        for takes_screen in [InputRequest::Scroll { reference: 100, dx: 0, dy: 1 }, InputRequest::Move { shot: 100, x: 1, y: 1 }, InputRequest::TypeFocused { window: 100, text: "t".into() }, InputRequest::Focus { window: 100 }] {
            assert!(matches!(begin_input(&ask, &gate, &takes_screen).unwrap().1, ScreenStep::Ask(_)), "{takes_screen:?}");
        }
        assert_eq!(gate.screen_allowed(Screen::Ask, "turn-1"), None);
        assert_eq!(gate.screen_allowed(Screen::Allow, "turn-1"), Some(true));

        assert_eq!(refusal(gate.answer_screen("turn-1", AskOutcome::Paused)).0, ErrorCode::ApprovalPending);
        assert_eq!(refusal(gate.answer_screen("turn-1", AskOutcome::Answered(ApprovalAnswer::Cancel))), (ErrorCode::Declined, "No answer arrived about showing this task on screen.".into(), Some(Effect::NotDispatched)));
        assert_eq!(gate.screen_allowed(Screen::Ask, "turn-1"), None, "no answer is not a choice");

        // Kept in the background: refused for the rest of the turn without asking again.
        let kept = "The user chose to keep this task in the background, so nothing may bring a window to the front until the turn ends. Act on controls by reference (press, set-value, type --ref) and use screenshots to look; if the task cannot be done that way, tell the user.";
        assert_eq!(refusal(gate.answer_screen("turn-1", AskOutcome::Answered(ApprovalAnswer::Decline))), (ErrorCode::Declined, kept.into(), Some(Effect::NotDispatched)));
        assert_eq!(refusal(begin_input(&ask, &gate, &key)), (ErrorCode::Declined, kept.into(), Some(Effect::NotDispatched)));
        assert_eq!(begin_input(&ask, &gate, &press(100)).unwrap().1, ScreenStep::Proceed);
        // A ref action whose fallback needs the window in front is refused at the helper's check.
        let (resolved, _) = begin_input(&ask, &gate, &press(100)).unwrap();
        let input = CheckedInput { request: &press(100), turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
        let front = DispatchTarget { foreground: true, ..native(NOTES, Some(identity("button", "Done"))) };
        assert_eq!(refusal(final_check(&ask, &gate, &input, Some(&front))), (ErrorCode::Declined, "This control can only be operated with its window in front, which this task may not do. Act on other controls by reference, or ask the user.".into(), Some(Effect::NotDispatched)));
        assert_eq!(final_check(&ask, &gate, &input, Some(&native(NOTES, Some(identity("button", "Done"))))), Ok(()));

        // The next turn is asked again; shown on screen holds for that turn.
        let next = turn("turn-2", true);
        let ask = Live { turn: Some(&next), ..ask };
        assert!(matches!(begin_input(&ask, &gate, &key).unwrap().1, ScreenStep::Ask(_)));
        assert_eq!(gate.answer_screen("turn-2", AskOutcome::Answered(ApprovalAnswer::AcceptForSession)), Ok(()));
        assert_eq!(begin_input(&ask, &gate, &key).unwrap().1, ScreenStep::Proceed);
        let input = CheckedInput { turn_id: "turn-2", ..input };
        assert_eq!(final_check(&ask, &gate, &input, Some(&front)), Ok(()));
    }

    #[test]
    fn background_input_to_an_unfocused_window_does_not_ask_for_the_screen() {
        let mut gate = ThreadGate::starting_at(100);
        let listed = gate.targets.record_windows(&[DriverWindow { focused: false, ..driver_window("w1", "Notes", 70, "n") }, driver_window("w2", "Mail", 71, "m")]);
        let own = own();
        let full = turn("turn-1", true);
        let ask = Live { screen: Screen::Ask, background_input: true, ..live(Some(&full), &own) };
        let key = |window: u64| InputRequest::Key { window, keys: "enter".into() };
        assert_eq!(begin_input(&ask, &gate, &key(listed[0].id)).unwrap().1, ScreenStep::Proceed);
        // The focused window is already in front of the user, and focus always takes the screen.
        assert!(matches!(begin_input(&ask, &gate, &key(listed[1].id)).unwrap().1, ScreenStep::Ask(_)));
        assert!(matches!(begin_input(&ask, &gate, &InputRequest::Focus { window: listed[0].id }).unwrap().1, ScreenStep::Ask(_)));
        let foreground_only = Live { background_input: false, ..ask };
        assert!(matches!(begin_input(&foreground_only, &gate, &key(listed[0].id)).unwrap().1, ScreenStep::Ask(_)));
    }

    #[test]
    fn the_final_check_rereads_everything_before_input() {
        let mut gate = observed();
        let own = own();
        let full = turn("turn-1", true);
        let now = live(Some(&full), &own);
        let request = press(100);
        let (resolved, _) = begin_input(&now, &gate, &request).unwrap();
        let input = CheckedInput { request: &request, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
        let done = native(NOTES, Some(identity("button", "Done")));
        assert_eq!(final_check(&now, &gate, &input, None), Ok(()));
        assert_eq!(final_check(&now, &gate, &input, Some(&done)), Ok(()));

        // The setting changed.
        let observe = Live { mode: Mode::Observe, ..now };
        assert_eq!(refusal(final_check(&observe, &gate, &input, Some(&done))), (ErrorCode::ObserveOnly, "Computer use no longer allows input.".into(), Some(Effect::NotDispatched)));
        assert_eq!(refusal(final_check(&Live { mode: Mode::Off, ..now }, &gate, &input, None)).0, ErrorCode::ObserveOnly);

        // The turn ended, or another one runs: an approved action says declined, an unasked one says no turn.
        let asked = CheckedInput { asked: true, ..input.clone() };
        let next = turn("turn-2", true);
        for ended in [Live { turn: None, ..now }, Live { turn: Some(&next), ..now }] {
            assert_eq!(refusal(final_check(&ended, &gate, &input, None)), (ErrorCode::NoTurn, "The turn ended before the action ran.".into(), Some(Effect::NotDispatched)));
            assert_eq!(refusal(final_check(&ended, &gate, &asked, None)).0, ErrorCode::Declined);
        }

        // An approval opened anywhere since: even an action the user just approved waits.
        let paused = Live { approval_pending: true, ..now };
        assert_eq!(refusal(final_check(&paused, &gate, &asked, Some(&done))).0, ErrorCode::ApprovalPending);

        // The thread left Full access mid-action: an input that was not asked about would now ask.
        let sandboxed = turn("turn-1", false);
        let stricter = Live { turn: Some(&sandboxed), ..now };
        assert_eq!(refusal(final_check(&stricter, &gate, &input, None)), (ErrorCode::Declined, "The thread now requires approval; request the action again.".into(), Some(Effect::NotDispatched)));
        assert_eq!(final_check(&stricter, &gate, &asked, None), Ok(()));
        assert_eq!(final_check(&Live { approvals: Approvals::Risky, ..stricter }, &gate, &input, None), Ok(()));
        // "Never" became "risky" under a destructive action that had not asked.
        let destructive = CheckedInput { destructive: true, ..input.clone() };
        assert_eq!(final_check(&Live { approvals: Approvals::Never, ..now }, &gate, &destructive, None), Ok(()));
        assert_eq!(refusal(final_check(&Live { approvals: Approvals::Risky, ..now }, &gate, &destructive, None)).0, ErrorCode::Declined);

        // The native target the helper is about to act on.
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&native("wab12.9", None)))), (ErrorCode::Declined, "The native target changed before dispatch.".into(), Some(Effect::NotDispatched)));
        let mut vault = native(NOTES, None);
        vault.window.app = "1Password".into();
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&vault))).0, ErrorCode::ProtectedApp);
        let mut own_window = native(NOTES, None);
        own_window.window.pid = 4242;
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&own_window))).0, ErrorCode::ProtectedApp);
        let mut tab = native(NOTES, None);
        (tab.window.app, tab.window.title) = ("Safari".into(), "PandaCode".into());
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&tab))).0, ErrorCode::ProtectedApp);

        // The control was relabelled to something destructive after the up-front check.
        let relabelled = native(NOTES, Some(identity("button", "Delete")));
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&relabelled))), (ErrorCode::StaleRef, "The target now looks destructive; look again and request approval.".into(), Some(Effect::NotDispatched)));
        assert_eq!(final_check(&now, &gate, &destructive, Some(&relabelled)), Err(not_asked()), "a destructive action that was not asked about is refused before the label is compared");
        assert_eq!(final_check(&now, &gate, &CheckedInput { asked: true, ..destructive.clone() }, Some(&relabelled)), Ok(()));
        assert_eq!(final_check(&Live { approvals: Approvals::Never, ..now }, &gate, &input, Some(&relabelled)), Ok(()));
        // Compared as the agent saw it: clipped to what the helper reports.
        let long = native(NOTES, Some(identity("button", &format!("{} delete", "a".repeat(200)))));
        assert_eq!(final_check(&now, &gate, &input, Some(&long)), Ok(()));

        // The target went stale between the checks.
        gate.targets.record_observation(100, &[element("eab12.5", "button", "Done")]);
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&done))).0, ErrorCode::StaleRef);
    }

    fn not_asked() -> ComputerUseError {
        ComputerUseError { code: ErrorCode::Declined, message: "The thread now requires approval; request the action again.".into(), effect: Some(Effect::NotDispatched) }
    }

    #[test]
    fn the_final_check_refuses_a_target_that_is_no_longer_the_one_checked() {
        let mut gate = observed();
        let own = own();
        let full = turn("turn-1", true);
        let now = live(Some(&full), &own);
        let click = InputRequest::Click { shot: 100, x: 10, y: 10, button: None, count: None };
        let (resolved, _) = begin_input(&now, &gate, &click).unwrap();
        let input = CheckedInput { request: &click, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
        let key = InputRequest::Key { window: 100, keys: "enter".into() };
        let (window, _) = begin_input(&now, &gate, &key).unwrap();
        let key_input = CheckedInput { request: &key, turn_id: "turn-1", resolved: &window, asked: false, destructive: false };

        // A click that now lands on a destructive control; a move over one does not matter.
        let on_delete = DispatchTarget { foreground: true, ..native(NOTES, Some(identity("button", "Delete"))) };
        assert_eq!(refusal(final_check(&now, &gate, &input, Some(&on_delete))), (ErrorCode::StaleShot, "The target now looks destructive; look again and request approval.".into(), Some(Effect::NotDispatched)));
        let hover = InputRequest::Move { shot: 100, x: 10, y: 10 };
        assert_eq!(final_check(&now, &gate, &CheckedInput { request: &hover, ..input.clone() }, Some(&on_delete)), Ok(()));

        // The window was closed and listed again under the same handle: a new id, so not the target that was checked.
        gate.targets.record_windows(&[]);
        gate.targets.record_windows(&[driver_window(NOTES, "Notes", 70, "Shopping")]);
        assert_eq!(refusal(final_check(&now, &gate, &key_input, None)).0, ErrorCode::UnknownWindow);
        assert_eq!(refusal(final_check(&now, &gate, &input, None)).0, ErrorCode::StaleShot);
        // The same window id behind another handle, or another shot of it.
        let moved = ResolvedTarget::Window { window: WindowRecord { handle: "wzz.1".into(), ..gate.targets.window(102).unwrap().clone() } };
        let stale = CheckedInput { request: &InputRequest::Key { window: 102, keys: "enter".into() }, resolved: &moved, ..key_input.clone() };
        assert_eq!(refusal(final_check(&now, &gate, &stale, None)), (ErrorCode::StaleRef, "The target changed since this action was checked; look again.".into(), Some(Effect::NotDispatched)));
    }

    #[test]
    fn no_input_while_any_approval_is_pending_anywhere() {
        let mut pending = PendingApprovals::default();
        assert!(!pending.any_pending(Some(false)));
        assert!(pending.any_pending(Some(true)), "an approval storage shows in another thread");
        assert!(pending.any_pending(None), "unreadable storage counts as pending");

        // The gate's own card, in any thread.
        let request = pending.open("thread-a", "turn-1", Some(false)).expect("opens");
        assert!(request.starts_with("computer-use:"));
        assert!(pending.any_pending(Some(false)));
        assert_eq!(pending.open("thread-b", "turn-9", Some(false)), None, "two cards are never open at once");
        assert!(pending.close(&request) && !pending.close(&request));
        assert!(!pending.any_pending(Some(false)));
        assert_eq!(pending.open("thread-a", "turn-1", Some(true)), None);
        assert_eq!(pending.open("thread-a", "turn-1", None), None);

        // A provider's approval, tracked before storage shows it; a question for the user is not one.
        pending.track_provider("thread-b", "req-1", Some("turn-9"), true, false);
        assert!(pending.any_pending(Some(false)));
        pending.track_provider("thread-b", "req-1", Some("turn-9"), false, false);
        pending.track_provider("thread-b", "ask-1", Some("turn-9"), true, true);
        pending.track_provider("thread-b", "computer-use:abc", Some("turn-9"), true, false);
        assert!(!pending.any_pending(Some(false)));

        // A provider request still tracked for an ended turn will never be answered.
        pending.track_provider("thread-b", "req-2", Some("turn-9"), true, false);
        pending.track_provider("thread-b", "req-3", None, true, false);
        pending.track_provider("thread-c", "req-4", Some("turn-5"), true, false);
        assert_eq!(pending.end_turn("thread-b", Some("turn-8")), Vec::<String>::new());
        assert!(pending.any_pending(Some(false)));
        pending.end_turn("thread-b", Some("turn-9"));
        pending.end_turn("thread-c", None);
        assert!(!pending.any_pending(Some(false)));
        pending.track_provider("thread-c", "req-5", Some("turn-5"), true, false);
        assert_eq!(pending.release_thread("thread-b"), Vec::<String>::new());
        assert!(pending.any_pending(Some(false)));
        assert_eq!(pending.release_thread("thread-c"), Vec::<String>::new());
        assert!(!pending.any_pending(Some(false)));
    }

    #[test]
    fn an_approval_is_answered_once_by_its_own_thread_and_turn() {
        let mut pending = PendingApprovals::default();
        let accept = ApprovalAnswer::Accept;
        assert_eq!(pending.respond("thread-a", "req-1", Some("turn-1"), accept), (ApprovalResponse::NotOwned, None));
        assert_eq!(pending.respond("thread-a", "computer-use:gone", Some("turn-1"), accept), (ApprovalResponse::Stale, None));

        let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
        assert_eq!(pending.respond("thread-b", &request, Some("turn-1"), accept), (ApprovalResponse::Stale, None), "another thread cannot answer it");
        assert!(pending.any_pending(Some(false)));
        assert_eq!(pending.respond("thread-a", &request, Some("turn-1"), accept), (ApprovalResponse::Handled, Some(accept)));
        assert_eq!(pending.respond("thread-a", &request, Some("turn-1"), accept), (ApprovalResponse::Stale, None));
        assert!(!pending.any_pending(Some(false)));

        // The turn it belonged to is no longer the running one: cancelled, never accepted.
        for running in [Some("turn-2"), None] {
            let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
            assert_eq!(pending.respond("thread-a", &request, running, accept), (ApprovalResponse::Stale, Some(ApprovalAnswer::Cancel)));
            assert!(!pending.any_pending(Some(false)));
        }

        // Ending the turn, stopping the session or shutting down cancels what is open.
        let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
        assert_eq!(pending.end_turn("thread-a", Some("turn-2")), Vec::<String>::new());
        assert_eq!(pending.end_turn("thread-b", None), Vec::<String>::new());
        assert_eq!(pending.end_turn("thread-a", Some("turn-1")), [request]);
        let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
        assert_eq!(pending.end_turn("thread-a", None), [request]);
        let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
        assert_eq!(pending.release_thread("thread-a"), [request]);
        let request = pending.open("thread-a", "turn-1", Some(false)).unwrap();
        pending.track_provider("thread-b", "req-1", None, true, false);
        assert_eq!(pending.release_all(), [request]);
        assert!(!pending.any_pending(Some(false)));
    }

    #[test]
    fn helper_failures_say_what_the_os_may_have_received() {
        let failure = |kind, dispatched| DriverFailure::new(kind, "why", dispatched);
        let mapped = |kind, dispatched, input| {
            let error = driver_error_to_computer_use_error(&failure(kind, dispatched), input);
            (error.code, error.message, error.effect)
        };
        assert_eq!(mapped(FailureKind::Unavailable, Dispatched::No, false), (ErrorCode::DriverUnavailable, "why".into(), None));
        assert_eq!(mapped(FailureKind::PermissionAccessibility, Dispatched::No, true), (ErrorCode::AccessibilityMissing, "why".into(), Some(Effect::NotDispatched)));
        assert_eq!(mapped(FailureKind::PermissionScreen, Dispatched::No, false), (ErrorCode::ScreenRecordingMissing, "why".into(), None));
        assert_eq!(mapped(FailureKind::Malformed, Dispatched::Unknown, true), (ErrorCode::DriverMalformed, "why".into(), Some(Effect::DispatchedUnknown)));
        assert_eq!(mapped(FailureKind::Stale, Dispatched::No, true), (ErrorCode::StaleRef, "The element changed since it was observed; observe the window again.".into(), Some(Effect::NotDispatched)));
        assert_eq!(mapped(FailureKind::Failed, Dispatched::Yes, true), (ErrorCode::DriverFailed, "why".into(), Some(Effect::Dispatched)));
        assert_eq!(mapped(FailureKind::Timeout, Dispatched::Unknown, true), (ErrorCode::DriverFailed, "why The input may have been delivered; observe before retrying.".into(), Some(Effect::DispatchedUnknown)));
        assert_eq!(mapped(FailureKind::Timeout, Dispatched::Unknown, false), (ErrorCode::DriverFailed, "why".into(), None));
        // A policy refusal carries the gate's own code back; without one it reads as declined.
        assert_eq!(mapped(FailureKind::Policy, Dispatched::No, true).0, ErrorCode::Declined);
        let paused = DriverFailure { code: Some(ErrorCode::ApprovalPending), ..failure(FailureKind::Policy, Dispatched::No) };
        assert_eq!(driver_error_to_computer_use_error(&paused, true).code, ErrorCode::ApprovalPending);
        assert_eq!(cancelled_before_dispatch(), ComputerUseError { code: ErrorCode::Declined, message: "The action was cancelled before dispatch.".into(), effect: Some(Effect::NotDispatched) });
        assert_eq!(internal_error().message, "PandaCode hit an internal error running this command.");
    }

    #[test]
    fn a_stale_target_is_retired_and_says_how_to_find_it_again() {
        let own = own();
        let stale = DriverFailure::new(FailureKind::Stale, "changed", Dispatched::No);
        let restarted = DriverFailure { reason: Some(FailureReason::Restarted), ..stale.clone() };
        let resolve = |gate: &ThreadGate, request: &InputRequest| resolve_input_target(&gate.targets, request, &own).unwrap();
        let report = |gate: &mut ThreadGate, target: &ResolvedTarget, failure: &DriverFailure| {
            let error = input_failure(&mut gate.targets, target, failure);
            (error.code, error.message, error.effect)
        };
        let key = InputRequest::Key { window: 100, keys: "enter".into() };
        let click = InputRequest::Move { shot: 100, x: 1, y: 1 };

        // The control changed: nothing is retired, the agent observes again.
        let mut gate = observed();
        let element = resolve(&gate, &press(100));
        assert_eq!(report(&mut gate, &element, &stale), (ErrorCode::StaleRef, "The element changed since it was observed; observe the window again.".into(), Some(Effect::NotDispatched)));
        assert!(gate.targets.window(100).is_some());
        // The window moved: its shot stops accepting coordinates, the window stays.
        let shot = resolve(&gate, &click);
        assert_eq!(report(&mut gate, &shot, &stale), (ErrorCode::StaleShot, "The window moved, resized or closed since shot 100; take a new screenshot.".into(), Some(Effect::NotDispatched)));
        assert_eq!(refusal(resolve_input_target(&gate.targets, &click, &own)).0, ErrorCode::StaleShot);
        assert!(gate.targets.window(100).is_some());
        // A window the helper can no longer identify.
        let window = resolve(&gate, &key);
        assert_eq!(report(&mut gate, &window, &stale), (ErrorCode::UnknownWindow, "The driver can no longer identify window 100. Run `viewcode-computer list-windows`.".into(), Some(Effect::NotDispatched)));
        assert_eq!(gate.targets.window(100), None);

        // After a helper restart every handle of the old one is gone, the window's included.
        let mut gate = observed();
        let element = resolve(&gate, &press(100));
        assert_eq!(report(&mut gate, &element, &restarted), (ErrorCode::StaleRef, "The computer-use driver restarted; run `viewcode-computer list-windows`, then observe again.".into(), Some(Effect::NotDispatched)));
        assert_eq!(gate.targets.window(100), None);
        let mut gate = observed();
        let shot = resolve(&gate, &click);
        assert_eq!(report(&mut gate, &shot, &restarted), (ErrorCode::UnknownWindow, "The computer-use driver restarted; run `viewcode-computer list-windows` again.".into(), Some(Effect::NotDispatched)));
        assert_eq!(gate.targets.window(100), None);

        // Observing and screenshots: a stale handle retires the window, anything else is reported as is.
        let mut gate = observed();
        let window = gate.targets.window(100).unwrap().clone();
        assert_eq!(observation_failure(&mut gate.targets, &window, &DriverFailure::new(FailureKind::PermissionScreen, "Grant Screen Recording.", Dispatched::No)), ComputerUseError { code: ErrorCode::ScreenRecordingMissing, message: "Grant Screen Recording.".into(), effect: None });
        assert!(gate.targets.window(100).is_some());
        assert_eq!(observation_failure(&mut gate.targets, &window, &stale), ComputerUseError { code: ErrorCode::UnknownWindow, message: "The driver can no longer identify window 100. Run `viewcode-computer list-windows`.".into(), effect: None });
        assert_eq!(gate.targets.window(100), None);
        // A failure that is not about staleness leaves the tables alone.
        let mut gate = observed();
        let shot = resolve(&gate, &click);
        let timeout = DriverFailure::new(FailureKind::Timeout, "The computer-use driver timed out.", Dispatched::Unknown);
        assert_eq!(report(&mut gate, &shot, &timeout).2, Some(Effect::DispatchedUnknown));
        assert!(resolve_input_target(&gate.targets, &click, &own).is_ok());
    }

    #[test]
    fn small_rules_around_observation() {
        let mut field = element("e1", "text field", "Search");
        field.value = Some("Milk and Eggs".into());
        assert!(matches_query(&field, " EGGS "));
        assert!(matches_query(&field, "sear"));
        assert!(matches_query(&field, "  "));
        assert!(!matches_query(&field, "bread"));
        assert!(!matches_query(&element("e2", "button", "Save"), "milk"));
        let dir = screenshots_dir(std::path::Path::new("state"), "thread/../a b:1");
        assert_eq!(dir, std::path::Path::new("state").join("computer-use").join("screenshots").join("thread_.._a_b_1"));
        assert_eq!((MAX_OBSERVED_ELEMENTS, DEFAULT_SCREENSHOT_SIZE, SCREENSHOTS_KEPT, SETTLE_DELAY_MS, APPROVAL_APP_NAME), (300, 1568, 8, 350, "Computer use"));
    }
}
