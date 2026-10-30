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
- 🔗 **Comparte** archivos y carpetas con un enlace público, y quítalo cuando quieras
- 🗂️ **Crea**, **mueve** y **renombra** carpetas; **mueve**, **renombra** y **copia** archivos (las copias se hacen en el servidor, no se descarga nada)
- 📊 Consulta tu **almacenamiento**: usado, libre y en la papelera
- 🗑️ **Borra** a la papelera, con comprobaciones de seguridad (confirmación para carpetas, la carpeta raíz no se puede borrar, los nombres ambiguos nunca se adivinan)

## 📦 Instalación

Con [Rust](https://rustup.rs) instalado:

```bash
cargo install o2cloud-cli
```

En cada [release de GitHub](https://github.com/danielperez9430/o2cloud-cli/releases) hay binarios ya compilados para macOS, Linux y Windows.

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
→ Opening login window (enter your phone number there)...
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
| 📂 `ls [carpeta]` | Lista una carpeta (`-t` en árbol, `-a` todos los archivos, `-l` fechas, `-s` orden, `-n` filtro, `--json`) |
| 🔍 `find <texto>` | Busca archivos y carpetas por nombre |
| ⬆️ `upload <archivo>` | Sube un archivo (`--folder <id>` para elegir dónde) |
| 📁 `upload-dir <dir>` | Sube una carpeta con todo su contenido |
| 🗜️ `upload-zip <dir>` | Comprime una carpeta y sube el `.zip` |
| ⬇️ `download <archivo>` | Descarga un archivo (`-o` para elegir la ruta local) |
| 🗑️ `rm <archivo o carpeta>` | Lo manda a la papelera (`-r` para carpetas, `-y` para no pedir confirmación) |
| 🔗 `share <archivo o carpeta>` | Crea un enlace público e imprime su URL |
| 📋 `shares` | Lista las carpetas compartidas |
| 🚫 `unshare <carpeta>` | Quita el enlace público de una carpeta |
| 📁 `mkdir <carpeta>` | Crea una carpeta (`-p` para crear también las carpetas padre que falten) |
| 📦 `mv <archivo o carpeta> <carpeta>` | Mueve un archivo o una carpeta a otra carpeta |
| 📑 `cp <archivo> <carpeta>` | Copia un archivo en el servidor (`--name` para darle otro nombre) |
| ✏️ `rename <archivo o carpeta> <nombre>` | Renombra un archivo o una carpeta |
| 📊 `quota` | Muestra el almacenamiento usado, libre y en la papelera |

Ejecuta `o2cloud <comando> --help` para ver todas las opciones.

### 🧩 Salida JSON

`ls`, `upload`, `upload-dir`, `upload-zip`, `mkdir`, `cp`, `status` y `quota` aceptan `--json` e imprimen un único valor JSON en stdout, para scripts y otras apps. Los mensajes de progreso siguen saliendo por stderr, y los errores son `Error: …` en stderr con código de salida 1.

```console
$ o2cloud upload ./pelicula.mkv --json 2>/dev/null
{ "id": 1234567890, "name": "pelicula.mkv", "folderid": 12345678, "size": 1048576 }
$ o2cloud mkdir -p /Copias --json 2>/dev/null
{ "id": 12345679, "path": "/Copias", "created": false }
```

- `upload` y `upload-zip`: `id`, `name`, `folderid`, `size`. Usa el `id` directamente: una subida nueva puede tardar un minuto en aparecer en `ls`.
- `upload-dir`: un array con un objeto por archivo: `local` (su ruta dentro del directorio), `name`, `folderid`, `size`, y `id`, o `error` si ese archivo falló.
- `mkdir`: `id`, `path` y `created` (`false` cuando `-p` la encontró ya creada).
- `cp`: `id` de la copia, `name`, `folderid`, `path` y `source` (el ID del original).
- `quota`: `quota`, `used`, `free`, `softdeleted` (en la papelera), en bytes, y `nolimit`.
- `status`: `logged_in`, `phone`, `since`, `session` (`active`, `expired` o `unknown`), `renews_silently` y `storage` (como en `quota`, o `null`).

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

o2cloud download /Documentos/informe.pdf          # se guarda como ./informe.pdf
o2cloud download 1234567890 -o ~/Desktop/copia.pdf
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
> **La sesión dura unas 24 horas** y no se renueva sola. Cuando caduca, el siguiente comando vuelve a abrir la ventana de login (o, si no tienes, vuelve a entrar con las [cookies del navegador](#️-login-sin-ventana-de-acceso)). `o2cloud status` te dice si la tuya sigue activa. El modo avanzado **Mantener la sesión sin SMS** de más abajo sí se renueva solo.

> [!WARNING]
> **Los enlaces compartidos son públicos**: cualquiera con la URL puede abrir el archivo o la carpeta. Compartir la carpeta raíz no está permitido, porque publicaría toda la cuenta.

- **Lo borrado va a la papelera**, así que se puede restaurar desde la web de O2 Cloud.
- **`download` nunca sobrescribe** un archivo local que ya exista. Sin `-o`, solo usa el nombre del archivo, nunca una carpeta enviada por el servidor.
- **`upload-dir` omite los archivos y carpetas ocultos** (`.git`, `.DS_Store`…), porque O2 Cloud los rechaza. `upload-zip` sí los incluye dentro del zip.
- **Enlaces simbólicos:** los enlaces a archivos se siguen; los enlaces a carpetas se omiten con un aviso, para que un enlace a una carpeta superior no provoque un bucle infinito.
- **No se sobrescribe nada:** `mkdir`, `mv`, `cp` y `rename` se niegan a crear un segundo archivo o carpeta con el mismo nombre en el mismo sitio.
- **Aún no soportado:** dejar de compartir un archivo, y copiar carpetas. De momento hazlo desde la web.

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

Las cookies se comprueban contra O2 Cloud antes de guardarlas. Trátalas como una contraseña: cualquiera que las tenga puede entrar en tu cuenta hasta que la sesión caduque. Como cualquier sesión, dura unas 24 horas; después, copia cookies nuevas y vuelve a iniciar sesión.

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

`o2cloud status` indica si tu sesión se renueva sola. Mantén el client secret fuera del repositorio y del historial de la terminal: usa mejor las variables de entorno que las flags `--oauth-client-id` / `--oauth-client-secret`, y no lo subas a ningún sitio.

## ⚙️ Cómo funciona

O2 Cloud funciona sobre **Synchronoss OneMediaHub**. `o2cloud` usa la misma API que la web de O2 Cloud, y por defecto hace el login de la misma forma: el login OAuth2 / OpenID Connect de Telefónica, con PKCE gestionado por el servidor de O2 Cloud, así que el login por defecto no necesita ningún secreto de cliente. El número de teléfono y el código SMS se escriben en la ventana de login, nunca en la terminal. (El modo opcional **Mantener la sesión sin SMS** de arriba usa en su lugar el cliente OAuth de la app móvil, que aportas tú, para mantener la sesión viva sin SMS.)

La sesión se guarda en:

| Sistema | Ruta |
|---|---|
| macOS | `~/Library/Application Support/o2cli/auth.json` |
| Linux | `~/.config/o2cli/auth.json` |
| Windows | `%APPDATA%\o2cli\auth.json` |

En macOS y Linux el archivo solo lo puede leer tu usuario (`0600`). `o2cloud logout` lo borra.

## 📄 Licencia

[MIT](./LICENSE)
