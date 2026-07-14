use hiveweb::app_mode::AppMode;

#[test]
fn builtin_registration_is_disabled_only_in_production() {
    assert!(AppMode::Development.should_register_builtins());
    assert!(AppMode::Test.should_register_builtins());
    assert!(!AppMode::Production.should_register_builtins());
}
