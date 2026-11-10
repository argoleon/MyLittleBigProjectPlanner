#!/usr/bin/env bash
# Creates the secret files of the compose stack (deploy/compose.yaml) in
# deploy/secrets/, with random values. Existing files are never overwritten,
# so the script is safe to run again (it only adds what is missing).
#
#   deploy/scripts/generate-secrets.sh [directory]     (`just compose-secrets`)
#
# The files are readable by the container users (mode 0644): Docker Compose
# mounts them as they are. The directory itself is private (0700).
set -euo pipefail

dir="${1:-$(dirname "$0")/../secrets}"
mkdir -p "$dir"
chmod 700 "$dir"

# 32 random characters among A-Z, a-z and 0-9 (safe in URLs and configs).
random() {
    local value=""
    while [ "${#value}" -lt 32 ]; do
        value+=$(head -c 64 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')
    done
    printf '%s' "${value:0:32}"
}

created=()
# write NAME VALUE: creates the file if it does not exist yet.
write() {
    local file="$dir/$1"
    if [ -e "$file" ]; then
        return
    fi
    printf '%s' "$2" > "$file"
    chmod 644 "$file"
    created+=("$1")
}

# Value of an existing (or just created) secret.
read_secret() { cat "$dir/$1"; }

write postgres_password "$(random)"
write mlbpp_owner_password "$(random)"
write mlbpp_app_password "$(random)"
write database_url "postgres://mlbpp_app:$(read_secret mlbpp_app_password)@postgres:5432/mlbpp"
write database_migration_url "postgres://mlbpp_owner:$(read_secret mlbpp_owner_password)@postgres:5432/mlbpp"
write pgbackrest_cipher_pass "$(random)"
write valkey_password "$(random)"
write valkey_url "redis://:$(read_secret valkey_password)@valkey:6379"
write seaweedfs_s3_config "$(printf '{"identities":[{"name":"mlbpp","credentials":[{"accessKey":"%s","secretKey":"%s"}],"actions":["Admin","Read","List","Tagging","Write"]}]}' "$(random)" "$(random)")"
write keycloak_db_password "$(random)"
write keycloak_admin_password "$(random)"
write grafana_admin_password "$(random)"
write bootstrap_admin_password "$(random)"
# 32 random bytes in base64: the AES-256 key of the secrets stored in the
# database (TOTP, OIDC client secrets).
write data_encryption_key "$(head -c 32 /dev/urandom | base64)"
# Not random: the operator's SMTP server. Until it is replaced, emails (password
# resets) stay queued and the worker logs why.
write smtp_url "smtp://smtp.example.invalid:587?tls=required"

if [ "${#created[@]}" -eq 0 ]; then
    echo "All secrets already exist in $dir."
else
    echo "Created in $dir: ${created[*]}"
    echo "Keep a copy of pgbackrest_cipher_pass outside this machine: backups cannot be restored without it."
    case " ${created[*]} " in
        *" smtp_url "*) echo "Write the URL of your SMTP server to $dir/smtp_url (see deploy/README.md)." ;;
    esac
    case " ${created[*]} " in
        *" bootstrap_admin_password "*)
            echo "The first platform super admin (BOOTSTRAP_ADMIN_EMAIL) signs in with the password in $dir/bootstrap_admin_password."
            ;;
    esac
fi
