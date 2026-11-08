#!/bin/sh
# Usuarios de MySQL por servicio con el mínimo privilegio, y libro de movimientos de solo agregar.
#
# Lo ejecuta MySQL al inicializar una base nueva (/docker-entrypoint-initdb.d). En una base que
# ya existe se corre UNA vez a mano (idempotente):
#   docker exec -e WEB_DB_PASSWORD -e YAPE_DB_PASSWORD machupeek-mysql \
#     sh /docker-entrypoint-initdb.d/02-service-users.sh
# y luego se definen WEB_DB_USER=machupeek_web y YAPE_DB_USER=machupeek_yape en el entorno de
# docker compose para que los servicios dejen de entrar como root.
set -eu

: "${MYSQL_ROOT_PASSWORD:?falta MYSQL_ROOT_PASSWORD}"
DB="${MYSQL_DATABASE:-machupeek}"

sql() {
    MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysql -uroot "$DB" -e "$1"
}

# Escapa comillas simples para usar la contraseña dentro de un literal SQL.
quote() {
    printf "%s" "$1" | sed "s/'/''/g"
}

# Libro de movimientos: lo crea también la web al arrancar; aquí se asegura antes de protegerlo.
sql "CREATE TABLE IF NOT EXISTS wallet_ledger (
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    account_id VARCHAR(96) NOT NULL,
    delta_cents BIGINT NOT NULL,
    balance_after_cents BIGINT NOT NULL,
    kind VARCHAR(32) NOT NULL,
    reference VARCHAR(128) NULL,
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    PRIMARY KEY (id),
    KEY idx_ledger_account (account_id, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
sql "CREATE TABLE IF NOT EXISTS user_notifications (
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    discord_id VARCHAR(32) NULL,
    message VARCHAR(1000) NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    created_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    sent_at DATETIME(3) NULL,
    PRIMARY KEY (id),
    KEY idx_notifications_pending (sent_at, id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"

# Nadie (ni la web) puede corregir o borrar un movimiento: los errores se corrigen con un
# movimiento nuevo, así el historial de cada sol queda intacto.
sql "CREATE TRIGGER IF NOT EXISTS wallet_ledger_no_update BEFORE UPDATE ON wallet_ledger
     FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'wallet_ledger es de solo agregar'"
sql "CREATE TRIGGER IF NOT EXISTS wallet_ledger_no_delete BEFORE DELETE ON wallet_ledger
     FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'wallet_ledger es de solo agregar'"

if [ -n "${WEB_DB_PASSWORD:-}" ]; then
    p=$(quote "$WEB_DB_PASSWORD")
    sql "CREATE USER IF NOT EXISTS 'machupeek_web'@'%' IDENTIFIED BY '$p'"
    sql "ALTER USER 'machupeek_web'@'%' IDENTIFIED BY '$p'"
    # Datos y las migraciones que hace al arrancar (CREATE/ALTER/INDEX); sin DROP, GRANT,
    # TRIGGER ni privilegios globales.
    sql "GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, INDEX, REFERENCES ON \`$DB\`.* TO 'machupeek_web'@'%'"
    echo "usuario machupeek_web listo"
fi

if [ -n "${YAPE_DB_PASSWORD:-}" ]; then
    p=$(quote "$YAPE_DB_PASSWORD")
    sql "CREATE USER IF NOT EXISTS 'machupeek_yape'@'%' IDENTIFIED BY '$p'"
    sql "ALTER USER 'machupeek_yape'@'%' IDENTIFIED BY '$p'"
    # El monitor solo registra pagos, actualiza el estado de los retiros y deja avisos. No
    # puede tocar saldos (faceit_users) ni el libro.
    sql "GRANT SELECT, INSERT, UPDATE ON \`$DB\`.yape_transactions TO 'machupeek_yape'@'%'"
    sql "GRANT SELECT, UPDATE ON \`$DB\`.yape_withdrawals TO 'machupeek_yape'@'%'"
    sql "GRANT INSERT ON \`$DB\`.user_notifications TO 'machupeek_yape'@'%'"
    sql "GRANT INSERT ON \`$DB\`.yape_audit_logs TO 'machupeek_yape'@'%'"
    sql "GRANT SELECT ON \`$DB\`.realtime_documents TO 'machupeek_yape'@'%'"
    echo "usuario machupeek_yape listo"
fi
