# o2cloud-cli

<p align="center">
  <img src="assets/logo.svg" alt="o2cloud-cli" width="600">
</p>

[![Crates.io](https://img.shields.io/crates/v/o2cloud-cli)](https://crates.io/crates/o2cloud-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)

CLI para O2 Cloud (Telefónica España) — gestiona tu almacenamiento cloud desde la terminal.

[English](README.md)

## Instalación

```bash
cargo install o2cloud-cli
```

O compilar desde fuente:

```bash
git clone https://github.com/danielperez9430/o2cloud-cli
cd o2cloud-cli
cargo build --release
./target/release/o2cloud --help
```

## Uso

```bash
# Autenticación (abre una ventana de login: introduce ahí tu número y el código SMS)
o2cloud login

# Listar archivos
o2cloud ls                  # raíz
o2cloud ls /DMHAIR          # carpeta por path
o2cloud ls -t               # vista de árbol
o2cloud ls -a               # todos los archivos (plano)

# Buscar
o2cloud find "consulta"     # búsqueda case-insensitive

# Subir
o2cloud upload archivo.txt
o2cloud upload-dir ./mi-carpeta      # recursivo
o2cloud upload-zip ./mi-carpeta      # zip + subir

# Descargar
o2cloud download 1195003130          # por ID; se guarda con su nombre en la nube, en el dir actual
o2cloud download /ruta/archivo.txt   # por ruta (o solo el nombre, si es único)
o2cloud download 1195003130 -o archivo.txt

# Borrar (papelera)
o2cloud rm 1195003130                # archivo por ID
o2cloud rm /ruta/archivo.txt         # archivo por ruta
o2cloud rm /ruta/carpeta -r          # carpeta recursiva (pide confirmación)
o2cloud rm /ruta/carpeta -r -y       # carpeta recursiva, sin confirmación

# Sesión
o2cloud status
o2cloud logout
```

### Notas

- `upload-dir ./mi-carpeta` crea `mi-carpeta/` en la carpeta de destino
  (o reutiliza la que ya exista) y sube todo dentro, recorriendo todas las
  subcarpetas. Los archivos y carpetas
  ocultos (`.git`, `.DS_Store`, …) se omiten porque O2 Cloud los rechaza.
- `upload-dir` y `upload-zip` siguen los enlaces simbólicos a archivos,
  pero **omiten los enlaces a carpetas** (con un aviso), para que un enlace
  que apunte a una carpeta padre no provoque un bucle infinito.
- `download` nunca sobrescribe un archivo local existente. Sin `-o`, solo
  se usa el nombre del archivo (se ignora cualquier ruta que envíe el servidor).

## Requisitos

- macOS, Linux o Windows
- Cuenta de O2 Cloud (Telefónica España)
- Linux: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev`
- Windows: el [runtime de WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (preinstalado en Windows 11 y en Windows 10 actualizado)

## Cómo funciona

O2 Cloud usa **Synchronoss OneMediaHub v31** como backend. La autenticación usa el mismo flujo que la web de O2 Cloud: el login OAuth2/OpenID Connect de Telefónica, con PKCE gestionado por el servidor de O2 Cloud (sin ningún secreto de cliente). El CLI abre una ventana de login para que introduzcas tu número y valides por SMS. Tras el login, la sesión se guarda en:

- macOS: `~/Library/Application Support/o2cli/auth.json`
- Linux: `~/.config/o2cli/auth.json`
- Windows: `%APPDATA%\o2cli\auth.json`

En macOS/Linux el archivo solo es legible por tu usuario (`0600`).

La sesión dura unas 24 horas. Cuando caduca, el siguiente comando vuelve a abrir la ventana de login.

## Licencia

MIT
