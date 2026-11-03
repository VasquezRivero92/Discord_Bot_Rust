# Casos de contraste Node vs Rust

Cada `*.toml` de este directorio (sin recursion, orden alfabetico) contiene tablas `[[case]]`.
Se ejecutan con:

    cargo run -p contrast -- --node http://127.0.0.1:3001 --rust http://127.0.0.1:15633 contrast/cases

Opciones: `--jwt-secret <s>` (o variable `CONTRAST_JWT_SECRET`) para firmar `cookie_jwt`, y `--only <texto>` para filtrar por nombre. Salida 0 = todo igual, 1 = hay diferencias, 2 = error de uso o de carga. Nunca pongas secretos en los casos.

Campos de `[[case]]` (los desconocidos se rechazan): `name` y `path` obligatorios; `method` (GET por defecto), `headers` (tabla), `body` (texto crudo), `cookie_jwt` (`user_id`, `username`, `avatar`, `is_admin`, `role`, `servidor`; se envia como `auth_token` y `admin_token`), `ignore_fields` (rutas con puntos y `*`, p. ej. `users.*.last_seen`, `data.0.id`), `ignore_headers`, `expect_status`.

Se compara: estado, cuerpo JSON normalizado (orden de claves irrelevante, `1` == `1.0`) y las cabeceras `content-type`, `cache-control`, `location` y `set-cookie` (solo nombre y atributos; sin valor ni `expires`).

Los metodos distintos de GET/HEAD/OPTIONS se ejecutan en secuencia (Node y luego Rust): un caso con escrituras debe ser autocontenido.

Ejemplo completo y anotado, verificado por los tests: `crates/contrast/tests/fixtures/example.toml`.
