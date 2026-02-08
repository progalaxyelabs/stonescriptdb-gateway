//! Password hashing and verification using bcrypt
//!
//! This module provides secure password hashing functionality for storing
//! user credentials in the identities.password_hash column.

use bcrypt::{hash, verify, BcryptError};

/// Default bcrypt cost factor for password hashing
const BCRYPT_COST: u32 = 12;

/// Hash a plain-text password using bcrypt with cost factor 12
///
/// # Arguments
/// * `plain` - The plain-text password to hash
///
/// # Returns
/// * `Result<String, BcryptError>` - The hashed password or an error
///
/// # Example
/// ```no_run
/// use stonescriptdb_gateway::auth::password::hash_password;
///
/// let hash = hash_password("my_secure_password").unwrap();
/// // Store hash in identities.password_hash column
/// ```
pub fn hash_password(plain: &str) -> Result<String, BcryptError> {
    hash(plain, BCRYPT_COST)
}

/// Verify a plain-text password against a bcrypt hash
///
/// # Arguments
/// * `plain` - The plain-text password to verify
/// * `hash` - The bcrypt hash to verify against (from identities.password_hash)
///
/// # Returns
/// * `Result<bool, BcryptError>` - True if password matches, false otherwise
///
/// # Example
/// ```no_run
/// use stonescriptdb_gateway::auth::password::verify_password;
///
/// let stored_hash = "$2b$12$..."; // from database
/// let is_valid = verify_password("user_input", stored_hash).unwrap();
/// if is_valid {
///     // Password is correct
/// }
/// ```
pub fn verify_password(plain: &str, hash: &str) -> Result<bool, BcryptError> {
    verify(plain, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_password() {
        let plain = "test_password_123";
        let hash = hash_password(plain).expect("Failed to hash password");

        // Bcrypt hash should start with $2b$ or $2a$ or $2y$
        assert!(hash.starts_with("$2"));

        // Bcrypt hash should be 60 characters long
        assert_eq!(hash.len(), 60);
    }

    #[test]
    fn test_verify_password_success() {
        let plain = "my_secure_password";
        let hash = hash_password(plain).expect("Failed to hash password");

        let result = verify_password(plain, &hash).expect("Failed to verify password");
        assert!(result, "Password verification should succeed");
    }

    #[test]
    fn test_verify_password_failure() {
        let plain = "correct_password";
        let wrong = "wrong_password";
        let hash = hash_password(plain).expect("Failed to hash password");

        let result = verify_password(wrong, &hash).expect("Failed to verify password");
        assert!(!result, "Password verification should fail for wrong password");
    }

    #[test]
    fn test_different_hashes_for_same_password() {
        let plain = "same_password";
        let hash1 = hash_password(plain).expect("Failed to hash password");
        let hash2 = hash_password(plain).expect("Failed to hash password");

        // Hashes should be different due to random salt
        assert_ne!(hash1, hash2, "Two hashes of the same password should differ");

        // Both should verify correctly
        assert!(verify_password(plain, &hash1).unwrap());
        assert!(verify_password(plain, &hash2).unwrap());
    }

    #[test]
    fn test_empty_password() {
        let plain = "";
        let hash = hash_password(plain).expect("Failed to hash empty password");

        assert!(verify_password(plain, &hash).unwrap());
        assert!(!verify_password("not_empty", &hash).unwrap());
    }
}
