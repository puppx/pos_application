-- v0.9.4: remember how long each PIN is (not the PIN), so the login screen
-- can log in as soon as enough digits are typed. Unknown (NULL) until the
-- PIN is next set or used.
ALTER TABLE users ADD COLUMN pin_len INTEGER;
