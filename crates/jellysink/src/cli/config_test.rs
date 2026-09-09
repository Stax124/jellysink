use jellysink_core::config::Field;

/// `config set` is the only writer of config.toml, so its help text is where a
/// user finds the key names; a new `Field` that is not listed there is invisible.
#[test]
fn the_cli_help_lists_every_config_key() {
    let main_rs = include_str!("../main.rs");
    for field in Field::ALL {
        assert!(
            main_rs.contains(field.name()),
            "src/main.rs should mention {:?} in the `config set` help",
            field.name()
        );
    }
}
