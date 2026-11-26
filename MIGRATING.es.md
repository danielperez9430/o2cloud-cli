# Migrar de la 0.x a la 1.0

La 1.0 cambia algunos comportamientos por defecto para que la CLI sea más segura y más fácil de usar desde scripts y apps. Casi todo el mundo solo notará que subir dos veces ya no crea copias. Si llamas a `o2cloud` desde un script u otro programa, lee esta página: aquí está todo lo que puede romper algo y qué hacer en cada caso.

> Read in English: [MIGRATING.md](MIGRATING.md)

## Resumen

| Tema | 0.x | 1.0 | Qué hacer |
|---|---|---|---|
| [Códigos de salida](#códigos-de-salida) | Todo error sale con `1` | Del `2` al `7` según el tipo de error, `1` para lo demás | Tratar cualquier código distinto de cero como error, igual que antes, o usar los nuevos |
| [Errores con `--json`](#errores-con---json) | Solo `Error: …` en stderr | También `{"error": {…}}` en stdout | Mirar el código de salida antes de leer stdout como resultado |
| [Sesión caducada](#sesión-caducada-y-ventana-de-login) | Abre la ventana de login a mitad de cualquier comando | Solo la abre sola en un terminal; si no, código `4` | Ejecutar `o2cloud login` al recibir un `4` |
| [Un archivo que ya está en la nube](#archivos-que-ya-están-en-la-nube) | Se vuelve a subir y O2 Cloud lo llama `nombre (1)` | Mismo tamaño: se salta. Otro tamaño: conflicto (`--on-conflict`) | Añadir `--on-conflict duplicate` para el comportamiento anterior, o tratar el código `5` |

Lo que no cambia:

- **Campos del JSON:** siguen estando los mismos que en la 0.x con `--json`, y significan lo mismo (`id`, `parts`, `skipped`, `action`…). La 1.0 solo añade campos nuevos.
- **Archivos en partes:** se siguen llamando `nombre.001`, `nombre.002`…
- **Configuración y sesión:** siguen en el mismo sitio, así que no hace falta volver a entrar.
- **Archivos locales:** nunca se sobrescriben.

## Códigos de salida

En la 0.x todo error salía con `1`. En la 1.0 el código dice qué falló:

| Código | Significado | `code` en el JSON |
|---|---|---|
| `0` | Hecho | — |
| `1` | Cualquier otra cosa | `api_error`, `io`, `config`, … |
| `2` | Algo que no se puede hacer tal como se pide: argumentos incorrectos, un nombre ambiguo o un cambio que O2 Cloud no permite | `invalid`, `ambiguous` |
| `3` | No encontrado | `not_found` |
| `4` | Sin sesión o sesión caducada | `not_logged_in`, `session_expired`, `auth_failed` |
| `5` | Conflicto: ya hay algo con ese nombre | `conflict` |
| `6` | Fallaron algunos archivos y el resto se hizo | `partial_failure` |
| `7` | Falló la red o O2 Cloud | `network`, `server_error`, `bad_response` |

**Qué hacer:**

- Si tu script solo mira si el código es cero o no, no cambia nada.
- Si mira exactamente si es `1`, que mire si es distinto de cero.
- Con `6` y `7` vale la pena reintentar: relanza el mismo comando (con `--skip-existing` si es `upload-dir`).

## Errores con `--json`

La línea `Error: …` en stderr es la misma que en la 0.x. Con `--json`, la 1.0 además escribe el error en stdout como un único valor JSON, en lugar del resultado:

```json
{ "error": { "code": "session_expired", "message": "Session expired: …", "retryable": false, "exit_code": 4 } }
```

`upload-dir` y `download-dir` escriben su resultado (una entrada por archivo) antes de decir que fallaron algunos archivos. En ese caso el error ya no se escribe también en stdout, así que stdout siempre tiene un solo valor JSON.

**Qué hacer:**

- No tomes como resultado cualquier JSON que salga por stdout: mira primero el código de salida.
- Si buscas palabras de un mensaje, usa el campo `code` en su lugar. Los mensajes pueden cambiar de redacción; los códigos no.

## Sesión caducada y ventana de login

En la 0.x, con la sesión caducada, cualquier comando abría la ventana de login y esperaba. Una app o un script no tenía forma de saberlo, salvo detectando `→ Opening login window` en stderr.

En la 1.0, la ventana solo se abre sola si hay alguien en un terminal (stdin es un terminal). Si no, el comando para con el código `4` y el mensaje `Session expired: …`. El login de tipo app (el del móvil) se sigue renovando solo, como antes.

**Qué hacer:**

- Con el código `4` (o `code` igual a `session_expired` o `not_logged_in`), ejecuta `o2cloud login` cuando te convenga y luego repite el comando.
- Para mantener el comportamiento de la 0.x, define `O2CLOUD_LOGIN_WINDOW=1`.

## Archivos que ya están en la nube

En la 0.x, `upload`, `upload-dir` y `upload-zip` sin `--skip-existing` lo subían todo, y O2 Cloud guardaba las dos copias, la nueva como `nombre (1)`. Lanzar un comando dos veces duplicaba los archivos.

En la 1.0:

- **Mismo nombre y tamaño:** se salta siempre. Relanzar un comando sigue donde se quedó.
- **Mismo nombre y otro tamaño:** es un conflicto, y lo decide `--on-conflict`:
  - `ask` (por defecto en un terminal): muestra el tamaño y la fecha de las dos copias y pregunta. `upload-dir` pregunta todo antes de subir nada.
  - `fail` (por defecto en otro caso: scripts, apps o mensajes enviados a un archivo): no lo sube y termina con el código `5`.
  - `skip`: deja la copia de la nube (`--skip-existing` hace lo mismo).
  - `overwrite`: reemplaza la copia de la nube. La antigua va a la papelera, pero solo cuando la nueva ya está subida.
  - `duplicate`: el comportamiento de la 0.x.

Con `--json` y `fail`, cada conflicto trae todo lo que una app necesita para preguntar a su usuario:

```json
{
  "local": "fotos/a.jpg", "name": "a.jpg", "folderid": 123, "size": 2400000,
  "modified": "2024-06-02T10:00:00Z", "action": "conflict", "skipped": true,
  "cloud": { "id": 456, "size": 2100000, "modified": "2024-05-01T09:30:00Z" }
}
```

Un archivo en partes lleva `parts` (una lista de IDs) en lugar de `id`.

**Qué hacer:**

- **Para el comportamiento de la 0.x,** añade `--on-conflict duplicate`.
- **En una app,** no pases `--on-conflict` la primera vez. Si sale el código `5`, enseña al usuario cada entrada `conflict` ("tienes a.jpg, 2,4 MB, 02/06/2024; en la nube hay 2,1 MB, 01/05/2024: ¿sobrescribir o ignorar?"). Luego relánzalo con `--on-conflict overwrite` o `--on-conflict skip`. Si las respuestas son distintas para cada archivo, puedes lanzarlo una vez por archivo.
- **En un script** que ya usaba `--skip-existing`, no cambia nada.

## Valores nuevos en campos que ya existían

La 1.0 añade valores a campos que la 0.x ya escribía. Un programa que rechace valores desconocidos debería aceptar estos:

| Comando | Campo | Valores nuevos |
|---|---|---|
| `upload-dir --json` | `action` | `conflict` |
| `upload-dir --dry-run --json` | `action` | `conflict` |
| `upload --json`, `upload-zip --json` | `action` (nuevo; solo cuando no es una subida normal) | `differs`, `conflict`, `updated` |
