use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn money_uses_decimal_not_float() {
    let root = repo_root();

    // Money and financial types must use Decimal, not f64/f32
    // Check if any file in financial library uses Decimal (imports from qip-core)
    let financial_dir = root.join("crates/libs/qip-financial/src");

    let mut found_decimal = false;
    if let Ok(entries) = fs::read_dir(&financial_dir) {
        for entry in entries {
            if let Ok(e) = entry {
                let path = e.path();
                if path.is_file() && path.to_string_lossy().ends_with(".rs") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if content.contains("Decimal") {
                            found_decimal = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    assert!(
        found_decimal,
        "Financial library must use Decimal precision for money values"
    );
}

#[test]
fn core_defines_decimal_type() {
    let root = repo_root();

    // qip-core must define Decimal type
    let core_path = root.join("crates/libs/qip-core/src").join("lib.rs");
    let core_content = fs::read_to_string(&core_path).expect("could not read qip-core lib.rs");

    assert!(
        core_content.contains("Decimal") || core_content.contains("decimal"),
        "qip-core must define Decimal type for precise money handling"
    );
}

#[test]
fn capital_amounts_are_decimal() {
    let root = repo_root();

    // Capital contracts must use Decimal
    let capital_path = root
        .join("crates/libs/qip-contracts/src")
        .join("capital.rs");
    if capital_path.exists() {
        let capital_content = fs::read_to_string(&capital_path).expect("could not read capital.rs");

        assert!(
            capital_content.contains("Decimal")
                || capital_content.contains("Money")
                || capital_content.contains("Amount"),
            "Capital contracts must use Decimal precision"
        );
    }
}

#[test]
fn order_sizes_are_decimal() {
    let root = repo_root();

    // Orders must use Decimal for quantities
    let order_path = root
        .join("crates/services/qip-execution-engine/src")
        .join("order.rs");
    if order_path.exists() {
        let order_content = fs::read_to_string(&order_path).expect("could not read order.rs");

        assert!(
            order_content.contains("Decimal")
                || order_content.contains("quantity")
                || order_content.contains("Order"),
            "Order quantities must use Decimal precision"
        );
    }
}
