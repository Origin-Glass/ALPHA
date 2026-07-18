ALTER TABLE oauth_transactions
    ADD COLUMN browser_nonce_hash bytea;

-- OAuth transactions are short-lived and cannot survive a deploy that adds browser binding.
DELETE FROM oauth_transactions;

ALTER TABLE oauth_transactions
    ALTER COLUMN browser_nonce_hash SET NOT NULL,
    ADD CHECK (octet_length(browser_nonce_hash) = 32);

CREATE OR REPLACE FUNCTION revoke_sessions_on_user_status_change()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.status IS DISTINCT FROM OLD.status THEN
        UPDATE sessions
        SET revoked_at = COALESCE(revoked_at, now())
        WHERE user_id = NEW.id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER users_revoke_sessions_after_status_change
AFTER UPDATE OF status ON users
FOR EACH ROW
WHEN (OLD.status IS DISTINCT FROM NEW.status)
EXECUTE FUNCTION revoke_sessions_on_user_status_change();
