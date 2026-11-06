use super::*;

#[test]
fn generation_reservation_is_monotonic_and_fails_closed() {
    let root = crate::utils::test_temp::unique_temp_path("generation-reservation");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(reserve_generation(&root).unwrap(), 1);
    assert_eq!(reserve_generation(&root).unwrap(), 2);
    std::fs::write(generation_path(&root), "invalid").unwrap();
    assert!(reserve_generation(&root).is_err());
    std::fs::remove_file(generation_path(&root)).unwrap();
    std::fs::create_dir(generation_path(&root)).unwrap();
    assert!(reserve_generation(&root).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}
