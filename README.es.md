# o2cloud-cli

<p align="center">
  <img src="assets/logo.svg" alt="o2cloud-cli" width="600">
</p>

<p align="center">
  <a href="https://crates.io/crates/o2cloud-cli"><img src="https://img.shields.io/crates/v/o2cloud-cli" alt="Crates.io"></a>
  <a href="https://github.com/danielperez9430/o2cloud-cli/actions/workflows/release.yml"><img src="https://github.com/danielperez9430/o2cloud-cli/actions/workflows/release.yml/badge.svg" alt="CI"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="Licencia: MIT"></a>
</p>

<p align="center">
  <b>Gestiona tu almacenamiento de O2 Cloud (Telefónica España) desde la terminal.</b><br>
  <a href="README.md">English</a>
</p>

---

## ✨ Funcionalidades

- 🔐 **Login** con tu número de teléfono y el código SMS, en la misma ventana de acceso que usa la web de O2 Cloud
- 📂 **Navega** por las carpetas en lista o en árbol, y **busca** por nombre
- ⬆️ **Sube** archivos sueltos, carpetas enteras o una carpeta como `.zip`
- ⬇️ **Descarga** por ID, ruta o nombre de archivo
- 🧩 Los archivos de **más de 4 GB** se suben en partes y se vuelven a unir al descargarlos, y las subidas interrumpidas se pueden **reanudar**
- 🔗 **Comparte** archivos y carpetas con un enlace público, y quítalo cuando quieras
- 🗂️ **Crea**, **mueve** y **renombra** carpetas; **mueve**, **renombra** y **copia** archivos (las copias se hacen en el servidor, no se descarga nada)
- 📊 Consulta tu **almacenamiento**: usado, libre y en la papelera
- 🗑️ **Borra** a la papelera, con comprobaciones de seguridad (confirmación para carpetas, la carpeta raíz no se puede borrar, los nombres ambiguos nunca se adivinan)

## 📦 Instalación

Con [Rust](https://rustup.rs) instalado:

```bash
cargo install o2cloud-cli
```

En cada [release de GitHub](https://github.com/danielperez9430/o2cloud-cli/releases) hay binarios ya compilados para macOS, Linux y Windows. El de Linux necesita glibc 2.39 o posterior (Ubuntu 24.04, Debian 13, Fedora 40 o más nuevos); en sistemas más antiguos, instálalo con `cargo install`.

<details>
<summary>🛠️ Compilar desde el código</summary>

```bash
git clone https://github.com/danielperez9430/o2cloud-cli
cd o2cloud-cli
cargo build --release
./target/release/o2cloud --help
```

</details>

### 🧰 Requisitos

| Plataforma | Necesita |
|---|---|
| macOS | Nada más |
| Linux | `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev` |
| Windows | El [runtime de WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalado en Windows 11 y en Windows 10 actualizado) |

También necesitas una cuenta de O2 Cloud (una línea móvil de O2 España).

## 🚀 Primeros pasos

```console
$ o2cloud login
→ Opening login window (web login: the session ends after a while without use; enter your phone number there)...
✓ Login successful — tokens saved
Logged in as +34 600000000

$ o2cloud ls
📁 /

  12345679    📁 Documentos/  8 files, 3.4 MB
  12345678    📁 Fotos/       240 files, 610.2 MB

  1234567890     1.2 MB  notas.pdf

  2 folders, 1 file, 614.8 MB in total

$ o2cloud upload-dir ./vacaciones --folder 12345678
→ Uploading ./vacaciones (24.0 MB) — 12 files, 1 dirs...
  📁 vacaciones/ (created id:12345690)
  ...
✓ Uploaded 12 files to ./vacaciones
```

## 📖 Comandos

| Comando | Qué hace |
|---|---|
| 🔐 `login` | Inicia sesión (abre una ventana de acceso) |
| ℹ️ `status` | Muestra la cuenta y si la sesión sigue activa |
| 🚪 `logout` | Borra la sesión guardada |
| 📂 `ls [carpeta]` | Lista una carpeta (`-t` en árbol, `-a` todos los archivos, `-l` fechas, `-s` orden, `-n` filtro, `--folder-only`, `--no-folder`, `--json`) |
| 🔍 `find <texto>` | Busca archivos y carpetas por nombre |
| ⬆️ `upload <archivo>… [/carpeta]` | Sube archivos (el último argumento, o `--folder`, dice dónde: un ID o una ruta como `/Fotos/2024`; `--on-conflict` para uno que ya está con otro tamaño) |
| 📁 `upload-dir <dir>` | Sube una carpeta con todo su contenido (`-j <n>` archivos a la vez, 2 por defecto; `--ext jpg,mp4` para solo esas extensiones; `--update` para reemplazar también los que cambiaron en local; `--dry-run` para ver solo lo que haría) |
| 🗜️ `upload-zip <dir>` | Comprime una carpeta y sube el `.zip` |
| ⬇️ `download <archivo>` | Descarga un archivo (`-o` para elegir la ruta local) |
| 📥 `download-dir <carpeta>` | Descarga una carpeta con todo su contenido, o `/` para toda la cuenta (`-o <dir>`, `-j <n>`, `--ext`, `--dry-run`) |
| 🗑️ `rm <archivo o carpeta>…` | Lo manda a la papelera (uno o varios; `-r` para carpetas, `-y` para no pedir confirmación) |
| 🔗 `share <archivo o carpeta>` | Crea un enlace público e imprime su URL |
| 📋 `shares` | Lista las carpetas compartidas |
| 🚫 `unshare <carpeta>` | Quita el enlace público de una carpeta |
| 📁 `mkdir <carpeta>` | Crea una carpeta (`-p` para crear también las carpetas padre que falten) |
| 📦 `mv <archivo o carpeta> <carpeta>` | Mueve un archivo o una carpeta a otra carpeta |
| 📑 `cp <archivo> <carpeta>` | Copia un archivo en el servidor (`--name` para darle otro nombre) |
| ✏️ `rename <archivo o carpeta> <nombre>` | Renombra un archivo o una carpeta |
| 🩹 `fix-jpeg <archivo o carpeta>` | Busca los JPEG cuyo perfil de color O2 Cloud no puede leer (`MED-1000`); `--apply` los repara sin tocar la imagen |
| 📊 `quota` | Muestra el almacenamiento usado, libre y en la papelera |

Ejecuta `o2cloud <comando> --help` para ver todas las opciones.

### 🧩 Salida JSON

> **¿Vienes de la 0.x?** La 1.0 cambia los códigos de salida, los errores con `--json`, la ventana de login y las subidas de archivos que ya están en la nube: mira [MIGRATING.es.md](MIGRATING.es.md).

`ls`, `find`, `upload`, `upload-dir`, `upload-zip`, `download`, `download-dir`, `fix-jpeg`, `mkdir`, `cp`, `share`, `shares`, `status` y `quota` aceptan `--json` e imprimen un único valor JSON en stdout, para scripts y otras apps. Los mensajes de progreso siguen saliendo por stderr. Un error es `Error: …` en stderr y, con `--json`, también un valor JSON en stdout en lugar del resultado (salvo que el resultado ya se hubiera impreso, como hacen `upload-dir` y `download-dir` antes de decir que algunos archivos fallaron):

```json
{ "error": { "code": "session_expired", "message": "Session expired: …", "retryable": false, "exit_code": 4, "run": "3fa9c1d2" } }
```

**Para apps: `--json-lines`.** `upload`, `upload-dir`, `upload-zip`, `download` y `download-dir` pueden escribir eventos según ocurren, un objeto JSON por línea en stdout:

```json
{"event":"start","command":"upload-dir","files":3,"bytes":40000004,"schema":1,"run":"3fa9c1d2"}
{"event":"progress","local":"big.bin","bytes":8126464,"size":20000000}
{"event":"file","local":"big.bin","action":"uploaded","ids":[1615362889],"id":1615362889,"size":20000000,"sha256":"9f2c…"}
{"event":"summary","files":3,"uploaded":3,"skipped":0,"conflicts":0,"failed":0}
```

- `start` abre la ejecución, con `files`, `bytes` y la versión del formato, `schema`.
- `progress` sale como mucho cuatro veces por segundo por archivo, con los bytes movidos hasta ese momento.
- `file` es la misma entrada que da `--json`. Lleva `ids` (siempre una lista) y, si el archivo se subió entero en esta ejecución, su `sha256`, calculado mientras se lee para enviarlo.
- `retry` avisa de que el servidor estaba ocupado, fallando o aún procesando un archivo, y la petición (o una subida) se repite.
- `summary` da los totales.
- `error` cierra una ejecución que falló, igual que con `--json`.

Los códigos de salida son los mismos que sin la opción.

Códigos de salida: `0` hecho; `2` una petición que no hará tal como se pide (argumentos incorrectos, un nombre ambiguo, un cambio que O2 Cloud no permite); `3` no encontrado; `4` sin sesión o sesión caducada (`code` `not_logged_in` / `session_expired`; entra con `o2cloud login`); `5` conflicto, ya hay algo con ese nombre (`conflict`); `6` fallaron algunos archivos y el resto se hizo (`partial_failure`; relánzalo con `--skip-existing`); `7` falló la red o O2 Cloud (`network`, `server_error`, `bad_response`; vale la pena reintentar más tarde); `1` cualquier otra cosa. `retryable` es `true` para 6 y 7.

Cuando la sesión ha caducado, la ventana de login solo se abre sola si estás en un terminal. Un script o una app recibe el código 4 y entra con `o2cloud login` cuando decida (con `O2CLOUD_LOGIN_WINDOW=1` se abre igualmente).

```console
$ o2cloud upload ./pelicula.mkv --json 2>/dev/null
{ "id": 1234567890, "name": "pelicula.mkv", "folderid": 12345678, "size": 1048576 }
$ o2cloud mkdir -p /Copias --json 2>/dev/null
{ "id": 12345679, "path": "/Copias", "created": false }
```

- `ls`: `path`, `folders` (`id`, `name`, y los `files` y el `size` que contienen, `null` con `--folder-only`) y `files` (`id`, `name`, `size`, `date`). `ls -a --json` es una lista de archivos con su `path`.
- `find`: `folders` (`id`, `name`, `path`) y `files` (`id`, `name`, `size`, `date`, `path`), como en `ls --json`.
- `upload` y `upload-zip`: `id`, `name`, `folderid`, `size` y `skipped` (`true` si `--skip-existing` lo encontró ya subido). Un archivo subido en partes lleva `parts` (un ID por parte) en lugar de `id`. Usa los IDs en los scripts: una subida nueva tarda hasta un minuto en aparecer en `ls`, `find` y las rutas, pero `rm`, `mv`, `rename`, `cp`, `share` y `download` aceptan su ID al momento (esperando unos segundos al servidor si hace falta).
- `upload-dir`: un array con un objeto por archivo: `local` (su ruta dentro del directorio), `name`, `folderid`, `size`, `skipped`, e `id` (o `parts`), o `error` si ese archivo falló. Un `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp` o `.heic` vacío no se envía, porque O2 Cloud rechaza las imágenes vacías: lleva `"skipped": true, "empty": true` y no tiene `id`. Cada entrada lleva además `action`: `uploaded`, `skipped`, `empty`, `failed`, `differs` (otro tamaño que en la nube; sin `--update` no se toca), y con `--update` `updated` (más `replaced`, los IDs de la versión antigua, ahora en la papelera) u `older`. Una entrada `older` lleva el `id` de la copia de la nube, y una `differs` su `id`, o `parts` si está en partes. Con `--dry-run` las entradas llevan `"dry_run": true`, no llevan `skipped`, y `action` es lo que haría: `upload`, `update` (más `replaces`), `resume`, `unchanged`, `differs`, `older`, `empty`, o `failed` para un archivo que supera los límites de O2 Cloud; `folderid` es `null` para una carpeta que aún no existe.
- `download`: `id` (o `parts`, para un archivo subido en partes), `name`, `path` (dónde se guardó, ruta absoluta) y `size`.
- `download-dir`: un array con un objeto por archivo: `id` (o `parts`), `name`, `cloud` (su ruta en la nube), `local` (su ruta dentro de la carpeta de destino, con `/`), `path` (dónde se guardó), `folderid`, `size`, `action` (`downloaded`, `skipped` o `failed`), `error` si falló, y `renamed: true` si su nombre local es distinto del de la nube. `path` es relativa si `-o` lo era. Con `--dry-run`, las entradas llevan `"dry_run": true` y `action` es lo que haría con lo que hay en disco: `download`, `resume` (una descarga cortada que continuaría), `skipped` (ya está) o `failed` (un archivo local con ese nombre que no sobrescribirá).
- `fix-jpeg`: un array con un objeto por JPEG con problemas: `path`, `problem` (`mislabelled` o `damaged`), `chunks`, `fixed`, y `error` si no se pudo reparar.
- `mkdir`: `id`, `path` y `created` (`false` cuando `-p` la encontró ya creada).
- `cp`: `id` de la copia, `name`, `folderid`, `path` y `source` (el ID del original); para un archivo en partes, `parts` y `source_parts` en lugar de `id` y `source`.
- `share`: `id` y `url` del enlace, `path` y `folder` (`true` para una carpeta; su `id` es el que acepta `unshare`).
- `shares`: un array de enlaces: `id`, `url`, `uses`, `folderid`, `path` y `foldername`.
- `quota`: `quota`, `used`, `free`, `softdeleted` (en la papelera), en bytes, y `nolimit`.
- `status`: `logged_in`, `phone`, `since`, `session` (`active`, `expired` o `unknown`), `renews_silently`, `login` (`app` para el login que se renueva sin SMS, `web` si no) y `storage` (como en `quota`, o `null`).

### 🎯 Cómo indicar archivos y carpetas

Donde un comando pide un archivo o una carpeta, puedes usar cualquiera de estas formas:

| Forma | Ejemplo | Significado |
|---|---|---|
| ID | `1234567890` | El que muestran `ls` y `find` |
| Ruta | `/Documentos/notas.pdf` | Empieza por `/`: ruta exacta desde la raíz |
| Nombre | `notas.pdf` | Sin `/`: se busca en toda la cuenta; tiene que ser único |

Si un nombre coincide con varios archivos, no se hace nada y el CLI lista las coincidencias para que elijas:

```console
$ o2cloud download notas.pdf
Error: 'notas.pdf' is ambiguous — 2 files have that name:
  1234567890    /notas.pdf
  1234567891    /Documentos/notas.pdf
Use the ID or the full path.
```

## 💡 Ejemplos

<details open>
<summary><b>📂 Navegar y buscar</b></summary>

```bash
o2cloud ls                     # carpeta raíz
o2cloud ls /Fotos/2024         # una carpeta por ruta
o2cloud ls 12345678            # una carpeta por ID
o2cloud ls -t                  # vista en árbol
o2cloud ls -a                  # todos los archivos, en lista
o2cloud ls -a /Fotos           # todos los archivos dentro de /Fotos
o2cloud ls -l -s date -r       # con fechas, los más recientes primero
o2cloud ls -s size -r --limit 10   # los 10 archivos más grandes
o2cloud ls -n '*.heic'         # solo los nombres que encajan con un patrón
o2cloud ls --json | jq '.files[].id'   # JSON para scripts
o2cloud ls /Fotos --folder-only       # rápido en una cuenta grande (sin totales de subcarpetas)
o2cloud ls --no-folder                # solo las fotos y vídeos que subió la app del móvil
o2cloud find factura           # búsqueda sin distinguir mayúsculas
```

</details>

<details open>
<summary><b>🔄 Subir y descargar</b></summary>

```bash
o2cloud upload ./informe.pdf                      # a la carpeta raíz
o2cloud upload ./informe.pdf --folder 12345679    # a una carpeta (por ID)
o2cloud upload-dir ./vacaciones                   # crea vacaciones/ y sube dentro
o2cloud upload-zip ./proyecto                     # sube proyecto.zip
o2cloud upload-dir ./vacaciones --skip-existing   # reanudar: salta lo que ya está
o2cloud upload-dir ./vacaciones --ext jpg,mp4     # solo esas extensiones
o2cloud upload-dir ./vacaciones --update --dry-run  # qué ha cambiado desde la última subida
o2cloud upload-dir ./vacaciones --update          # sube lo nuevo y lo que cambió
o2cloud upload ./disco.iso                        # más de 4 GB: disco.iso.001, disco.iso.002, …

o2cloud download /Documentos/informe.pdf          # se guarda como ./informe.pdf
o2cloud download 1234567890 -o ~/Desktop/copia.pdf
o2cloud download /disco.iso                       # une disco.iso.001, .002, … en disco.iso
o2cloud download-dir /Fotos -o ~/copia            # ~/copia/Fotos con todas sus subcarpetas
o2cloud download-dir / -o ~/cuenta-o2             # toda la cuenta (p. ej. antes de darte de baja)
o2cloud download-dir / -o ~/cuenta-o2 --skip-existing   # reanudar tras un corte
```

</details>

<details open>
<summary><b>🔗 Compartir</b></summary>

```console
$ o2cloud share /Fotos/2024
✓ Public link created for /Fotos/2024/ — anyone with it can access it:
https://cloud.o2online.es/share/f/TOKEN-DE-EJEMPLO
  (link 100001; remove it with `o2cloud unshare 100001`)

$ o2cloud shares
LINK ID     USES  URL                                               FOLDER
100001         3  https://cloud.o2online.es/share/f/TOKEN-DE-EJEMPLO  /Fotos/2024/

$ o2cloud unshare /Fotos/2024
✓ /Fotos/2024/ is no longer shared
```

La URL sale sola por la salida estándar, así que se puede usar en scripts: `url=$(o2cloud share /notas.pdf)`.

</details>

<details open>
<summary><b>🗂️ Organizar y borrar</b></summary>

```bash
o2cloud mkdir /Fotos/2024               # crea una carpeta
o2cloud mkdir -p /Fotos/2024/Verano     # creando también las que falten
o2cloud mv /notas.pdf /Documentos       # mueve un archivo...
o2cloud mv /Fotos/2024 /Archivo         # ...o una carpeta entera
o2cloud rename /Documentos/notas.pdf notas-2024.pdf
o2cloud rename /Archivo/2024 Fotos-2024
o2cloud cp /Documentos/notas.pdf /Copias                     # copia, en el servidor
o2cloud cp /Documentos/notas.pdf /Copias --name antiguo.pdf  # con otro nombre

o2cloud rm /Documentos/notas-2024.pdf   # un archivo
o2cloud rm /Fotos/antiguas -r           # una carpeta con su contenido (pregunta antes)
o2cloud rm /Fotos/antiguas -r -y        # lo mismo, sin preguntar (para scripts)
```

</details>

## ⚠️ A tener en cuenta

> [!NOTE]
> **La sesión caduca cuando pasa un rato sin usarse** (en nuestras pruebas, a las dos horas sin uso ya no valía; una en uso continuo, con un `upload-dir` largo, duró más de 14 horas), y no se renueva sola. Así que cuenta con que vuelva a salir la ventana de login después de un descanso. Cuando caduca, el siguiente comando vuelve a abrir la ventana de login (o, si no tienes, vuelve a entrar con las [cookies del navegador](#️-login-sin-ventana-de-acceso)). `o2cloud status` te dice si la tuya sigue activa. Si caduca a mitad de un comando, el comando se para con "Session expired"; un `upload-dir` largo se detiene en vez de fallar archivo a archivo, y al relanzarlo con `--skip-existing` pide login y sigue donde se quedó. El modo avanzado **Mantener la sesión sin SMS** de más abajo sí se renueva solo.

> [!WARNING]
> **Los enlaces compartidos son públicos**: cualquiera con la URL puede abrir el archivo o la carpeta. Compartir la carpeta raíz no está permitido, porque publicaría toda la cuenta.

- **Lo borrado va a la papelera**, así que se puede restaurar desde la web de O2 Cloud.
- **`download-dir`** recrea la carpeta dentro de `-o` (toda la cuenta, `/`, va directamente dentro, fotos de la app del móvil incluidas), con las subcarpetas vacías, y une los archivos que esta CLI subió en partes (los volúmenes `x.001`, `x.002`… propios, subidos como archivos, bajan tal cual). Cada archivo se escribe en un `.<id>.o2part` oculto a su lado y se renombra al terminar, con la fecha que tenía al subirlo; `--skip-existing` salta los que ya están con el mismo tamaño y reanuda una descarga cortada desde donde se quedó; `--dry-run` dice qué descargaría, reanudaría, saltaría o dejaría sin tocar, sin una petición por archivo. Los nombres que tu sistema no admite se ajustan (en Windows `<>:"|?*`, puntos finales, `CON`…), y a dos nombres que solo difieren en mayúsculas se les añade el ID del archivo, para no perder nada en un disco que no distingue mayúsculas; `--json` los marca como `renamed`.
- **`download` nunca sobrescribe** un archivo local que ya exista. Sin `-o`, solo usa el nombre del archivo, nunca una carpeta enviada por el servidor. Escribe el archivo en su sitio con un `.<nombre>.o2dl` oculto al lado mientras está incompleto: si se corta, el mismo comando lo reanuda desde donde se quedó (por la parte en la que iba, si está en partes). El archivo terminado queda con la fecha que tenía al subirlo.
- **`upload-dir` omite los archivos y carpetas ocultos** (`.git`, `.DS_Store`…), porque O2 Cloud los rechaza, y los que el sistema guarda para sí mismo (`Thumbs.db`, `desktop.ini`, `$RECYCLE.BIN`…). `upload-zip` sí los incluye dentro del zip.
- **Archivos de más de 4 GB** (lo máximo que O2 Cloud acepta en una subida): se suben en partes llamadas `nombre.001`, `nombre.002`, … `download nombre` las une, igual que `cat nombre.0* > nombre` (o 7-Zip) si descargas las partes de otra forma. `download` se niega a unirlas si falta alguna parte o alguna no encaja con las demás. `rm`, `mv`, `rename`, `cp` y `share` también aceptan el archivo por su nombre y actúan sobre todas sus partes; nombrar una sola parte (`nombre.002`) se rechaza, porque rompería el archivo (por su ID sí se puede). Subir un archivo en partes donde ya hay partes de otro con el mismo nombre se rechaza, salvo que `--skip-existing` reanude el mismo archivo.
- **`upload-dir --update`** sube también los archivos que cambiaron en local: con otro tamaño, o con una fecha de modificación más de 2 segundos posterior a la que tenía la copia de la nube al subirse. Primero sube la versión nueva y solo cuando eso ha ido bien manda la antigua a la papelera y le da a la nueva su nombre (si no, O2 Cloud la llamaría `nombre (1)`), así que una subida fallida deja intacta la versión antigua. Un archivo más antiguo en local que en la nube no se toca, aunque tenga otro tamaño: `--update` nunca pone una versión más antigua encima de una más nueva. Un archivo que pasó a superar los 4 GB de cada parte (o bajó de ellos) reemplaza sus partes (o el archivo entero) anteriores. Se comparan nombre, tamaño y fecha, no el contenido. `--dry-run` muestra lo que subiría o actualizaría sin cambiar nada: conviene usarlo primero en una carpeta grande.
- **Archivos que ya están:** `upload`, `upload-dir` y `upload-zip` saltan un archivo (o parte) que la carpeta de destino ya tiene con el mismo nombre y tamaño (no compara el contenido), así que relanzarlos sigue donde se quedaron. Un archivo con el mismo nombre pero otro tamaño (entero o en partes, también si pasó a superar los 4 GB de cada parte o bajó de ellos) es un conflicto, que decide `--on-conflict`:
  - `ask` (por defecto en un terminal): muestra el tamaño y la fecha de las dos copias y pregunta: sobrescribir, saltar, sobrescribir todos, saltar todos o salir. `upload-dir` pregunta todo antes de subir nada, para que luego siga solo.
  - `fail` (por defecto en otro caso: un script, una app, o con los mensajes enviados a un archivo): no lo sube y termina con el código 5; con `--json` la entrada lleva `"action": "conflict"`, el `size` y `modified` locales, y `cloud` con el `id` (o `parts`), `size` y `modified` de la copia de la nube, para que una app pregunte a su usuario y lo relance con `overwrite` o `skip`.
  - `skip` (o `--skip-existing`): deja la copia de la nube (`"action": "differs"`).
  - `overwrite`: sube la versión nueva y, solo si ha ido bien, manda la antigua a la papelera y le da a la nueva su nombre (`"action": "updated"`, con `replaced`).
  - `duplicate`: lo sube todo igualmente, como antes de la 1.0: O2 Cloud guarda las dos, la nueva como `nombre (1)`, y vuelve a subir los que ya estaban.

  `overwrite` y `upload-dir --update` también reemplazan un archivo del mismo tamaño que es más nuevo en local (un contenedor o una base de datos que cambió por dentro); con `--update` las demás opciones no se aplican, y nunca se pone una versión más antigua encima de una más nueva. Un `nombre.001` que es un archivo local junto a `nombre` nunca se toma por una parte suya. Las subidas nuevas tardan hasta un minuto en aparecer en los listados; `upload-dir` recuerda lo que se subió desde este ordenador y espera (hasta 2 minutos) a que aparezca antes de decidir, así que relanzarlo enseguida, por ejemplo tras caducar la sesión, no vuelve a subir esos archivos. Lo subido desde otro sitio hace unos instantes puede no verlo todavía.
- **O2 Cloud revisa lo que guarda:** los archivos vacíos o dañados, o cuyo contenido no coincide con la extensión (una foto HEIC llamada `.jpg`), se rechazan con `MED-1050` o `MED-1000`, siempre, así que reintentar no sirve. `upload-dir` ni siquiera envía los `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp` o `.heic` vacíos (los muestra como omitidos); el resto de archivos vacíos sí se suben.
- **JPEG rechazados con `MED-1000`:** algunos programas parten un perfil de color grande en trozos numerados todos como "1 de 1"; los visores lo toleran, O2 Cloud no. `o2cloud fix-jpeg ./fotos` los lista y `--apply` renumera los trozos en el sitio (unos 12 bytes; se conservan la imagen y la fecha del archivo). Cuando una subida falla por esto, el error lo dice.
- **Límites de O2 Cloud:** un nombre de archivo puede tener hasta 255 bytes en UTF-8 (255 letras sin acentos, pero solo 129 `ñ` o 87 `日`), un nombre de carpeta hasta 255 caracteres, y las carpetas pueden anidarse hasta 20 niveles por debajo de la raíz. No hay límite para la ruta completa, porque O2 Cloud guarda nombres, no rutas. `upload-dir` deja fuera (y avisa de) lo que no cabe en vez de pararse a medias, y `mkdir`, `rename` y `upload` dicen qué límite se ha superado.
- **Rutas largas en Windows:** las rutas locales de más de 260 caracteres funcionan sin activar `LongPathsEnabled`.
- **Enlaces simbólicos:** los enlaces a archivos se siguen; los enlaces a carpetas se omiten con un aviso, para que un enlace a una carpeta superior no provoque un bucle infinito.
- **No se sobrescribe nada:** `mkdir`, `mv`, `cp` y `rename` se niegan a crear un segundo archivo o carpeta con el mismo nombre en el mismo sitio.
- **Aún no soportado:** dejar de compartir un archivo, y copiar carpetas. De momento hazlo desde la web.

## 🐞 Reportar un problema

Si algo falla, ejecuta

```sh
o2cloud debug-report
```

y adjunta el `o2cloud-report-….zip` que genera a un [issue](https://github.com/danielperez9430/o2cloud-cli/issues). Lleva la versión, tu sistema, tu configuración (nunca las claves de sesión) y los logs de las 3 últimas ejecuciones, con cada nombre de archivo y carpeta sustituido por uno que conserva lo que hace falta para depurar nombres (extensión, longitud, acentos, puntos finales). No se envía nada; revísalo antes de adjuntarlo. `--stdout` lo muestra en pantalla, y `--include-names` mantiene los nombres.

Cada error termina con el ID de la ejecución (`… [run 3fa9c1d2]`), que identifica su log. Cada ejecución guarda un log pequeño en la carpeta de caché (`~/Library/Caches/o2cli/logs` en macOS, `~/.cache/o2cli/logs` en Linux, `%LOCALAPPDATA%\o2cli\logs` en Windows): las 10 últimas de los comandos que cambian o transfieren algo (`ls`, `find`, `status` y similares no guardan), de 5 MB como mucho cada una (50 MB con `--debug`; a partir de ahí, solo avisos y errores), y `o2cloud logout` las borra. `O2CLOUD_LOG_FILE=0` lo desactiva.

Las transferencias largas muestran una barra de progreso (archivos, bytes, velocidad, tiempo restante); `-v` añade una línea por archivo, y `-q` muestra solo avisos y errores. Para ver qué hace un comando paso a paso (cada petición, los reintentos y por qué cada archivo se sube, se salta o se deja), añade `--debug`: lo muestra en stderr y lo guarda entero en el log de la ejecución. `O2CLOUD_LOG=debug` hace lo mismo para apps y scripts, y `O2CLOUD_TRACE=1` muestra solo las peticiones. Las claves de sesión, los tokens y los enlaces de descarga firmados nunca aparecen, ni en pantalla ni en los logs; tu teléfono tampoco en logs, informes ni mensajes de error (`status` y `login` sí lo muestran, es su función).

## 🖥️ Login sin ventana de acceso

Normalmente `o2cloud login` abre una ventana pequeña para tu número y el código SMS. En una máquina sin entorno gráfico, o en un escritorio Linux donde esa ventana no se abre (por ejemplo en algunas configuraciones de Wayland), inicia sesión con las cookies de sesión de un navegador:

1. Inicia sesión en [cloud.o2online.es](https://cloud.o2online.es) en cualquier navegador.
2. Abre las herramientas de desarrollo (F12) → **Application** (Chrome/Edge) o **Almacenamiento** (Firefox) → **Cookies** → `https://cloud.o2online.es`.
3. Copia los valores de las cookies `JSESSIONID` y `validationKey` (el navegador muestra la segunda como `validationKey`, con K mayúscula), y pásalos a `login`:

```bash
o2cloud login --jsessionid "<JSESSIONID>" --validationkey "<validationkey>"
```

Para que no queden en el historial de la terminal, pásalos como variables de entorno:

```bash
export O2CLOUD_JSESSIONID="<JSESSIONID>" O2CLOUD_VALIDATIONKEY="<validationkey>"
o2cloud login
```

Las cookies se comprueban contra O2 Cloud antes de guardarlas. Trátalas como una contraseña: cualquiera que las tenga puede entrar en tu cuenta hasta que la sesión caduque. Como cualquier sesión, caduca cuando pasa un rato sin usarse; después, copia cookies nuevas y vuelve a iniciar sesión.

## 🧪 Avanzado: mantener la sesión sin SMS

> [!WARNING]
> **Avanzado, bajo tu responsabilidad.** Esto inicia sesión con el flujo OAuth de la **app móvil** en vez del flujo web. Necesita el **client id y el client secret** OAuth de la app —el CLI **no** los incluye, los aportas tú— y hace que el CLI se identifique ante Telefónica como la app oficial. Depende de un flujo no documentado que puede dejar de funcionar si Telefónica lo cambia o rota las credenciales. El `login` por defecto de arriba es el soportado.

La ventaja: este login guarda un refresh token, así que la sesión **se renueva sola sin pedir otro SMS** (unos 60 días). Eso hace viables las tareas programadas y desatendidas.

```bash
export O2CLOUD_OAUTH_CLIENT_ID="<client-id>"
export O2CLOUD_OAUTH_CLIENT_SECRET="<client-secret>"
o2cloud login
```

El número de teléfono y el código SMS se introducen una vez en la ventana de login. A partir de ahí, cuando la sesión caduca, el siguiente comando la renueva en silencio:

```console
$ o2cloud ls
→ Session expired, trying silent re-login...
✓ Session refreshed silently
📁 /
...
```

El client id y el secret se guardan con la sesión en `auth.json` (solo lo puede leer tu usuario; `logout` los borra), así que si alguna renovación falla, la ventana de login que se abre vuelve a usar este mismo login y la sesión se sigue renovando sola. `o2cloud login` también, hasta que hagas `logout`. `o2cloud status` indica si tu sesión se renueva sola. Mantén el client secret fuera del repositorio y del historial de la terminal: usa mejor las variables de entorno que las flags `--oauth-client-id` / `--oauth-client-secret`, y no lo subas a ningún sitio.

## ⚙️ Cómo funciona

O2 Cloud funciona sobre **Synchronoss OneMediaHub**. `o2cloud` usa la misma API que la web de O2 Cloud, y por defecto hace el login de la misma forma: el login OAuth2 / OpenID Connect de Telefónica, con PKCE gestionado por el servidor de O2 Cloud, así que el login por defecto no necesita ningún secreto de cliente. El número de teléfono y el código SMS se escriben en la ventana de login, nunca en la terminal. (El modo opcional **Mantener la sesión sin SMS** de arriba usa en su lugar el cliente OAuth de la app móvil, que aportas tú, para mantener la sesión viva sin SMS.)

La sesión se guarda en `auth.json`, y una caché de la lista de archivos de la cuenta (nombres, tamaños, fechas y carpetas; ver abajo) en `media-cache.json`:

| Sistema | Sesión | Caché de archivos |
|---|---|---|
| macOS | `~/Library/Application Support/o2cli/auth.json` | `~/Library/Caches/o2cli/media-cache.json` |
| Linux | `~/.config/o2cli/auth.json` | `~/.cache/o2cli/media-cache.json` |
| Windows | `%APPDATA%\o2cli\auth.json` | `%LOCALAPPDATA%\o2cli\media-cache.json` |

En macOS y Linux los dos archivos solo los puede leer tu usuario (`0600`). `o2cloud logout` los borra.

**Velocidad en cuentas grandes:** listar todos los archivos cuesta una petición por cada 1.000, así que los comandos lo evitan cuando pueden. Un archivo o carpeta indicado por ID o por ruta (`/docs/a.txt`) se busca por separado, y solo un nombre suelto (`a.txt`), `ls`, `upload-dir --skip-existing` y `rm` de una carpeta listan toda la cuenta. `ls --folder-only` lista solo la carpeta, sin los totales de sus subcarpetas; en la raíz deja fuera las fotos y vídeos que subió la app del móvil, que no tienen carpeta (`ls --no-folder` lista esas). Esos guardan la lista de archivos en la caché de arriba y, a partir de la primera vez, solo preguntan al servidor qué ha cambiado desde entonces (dos peticiones en total para un `ls`); la cuenta entera se vuelve a listar una vez por semana, o cuando la caché no se puede poner al día. Con `O2CLOUD_CACHE=0` siempre se lista todo y no se toca la caché. Las peticiones que solo leen (listados, el registro de cambios) se reintentan durante unos 30 s si el servidor da error, porque O2 Cloud tiene caídas cortas. Con `O2CLOUD_TRACE=1` se ve cada petición que envía un comando (método, ruta, estado y tiempo; nunca las claves de sesión) y cuántas han sido.

## 📄 Licencia

[MIT](./LICENSE)
