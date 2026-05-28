use mc_lint::{hot_path, hot_path_boundary};

#[hot_path(allow_branching, allow_validation)]
fn marked_hot_path(input: &[f64]) -> Option<f64> {
    if input.is_empty() {
        return None;
    }

    Some(input.iter().copied().sum())
}

#[hot_path_boundary]
fn marked_boundary(input: &[f64]) -> Option<f64> {
    marked_hot_path(input)
}

#[test]
fn public_attribute_reexports_compile() {
    assert_eq!(marked_boundary(&[1.0, 2.0, 3.0]), Some(6.0));
}
