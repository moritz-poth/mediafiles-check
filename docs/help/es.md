# Manual

## Qué hace MediaFiles Check

MediaFiles Check compara cada carpeta de `Avid MediaFiles/MXF` con su base de datos de Avid y muestra dónde no coinciden. Ayuda cuando faltan medios en Media Composer, aparecen sin conexión o después de copiar o mover archivos a mano.

La comprobación **solo lee**. Solo si tú lo pides, MediaFiles Check cambia el nombre de carpetas para Media Composer (ver abajo); nunca borra ni sobrescribe un archivo o una base de datos.

## Qué espera

En cada disco, también en discos de red:

- `Avid MediaFiles/MXF/<carpeta>/` con los archivos de medios `.mxf`
- los dos archivos de base de datos de Avid `msmFMID.pmr` y `msmMMOB.mdb` en cada carpeta
- las carpetas `Creating` y los archivos ocultos se ignoran

Los nombres de carpeta pueden ser cualquiera, p. ej. `1`, `Avid8.1` o `Mac Studio.1`.

## Comprobar discos

La página de inicio muestra cada disco conectado con `Avid MediaFiles/MXF`; el punto verde indica que el disco está listo. La lista se actualiza sola cuando conectas o expulsas un disco.

1. Activa los discos que quieres comprobar. Los discos nuevos se activan automáticamente.
2. Haz clic en **Comprobar discos**.

## Comprobar una carpeta

**Comprobar carpeta…** comprueba un lugar que elijas:

- un disco: todas sus carpetas MXF
- la propia carpeta `MXF`
- una sola carpeta de medios como `MXF/1`
- cualquier carpeta que contenga carpetas MXF, p. ej. una copia en un disco de respaldo

## El resultado

El resultado muestra cada carpeta con diferencias y lo que se encontró:

- **Archivos que no están en la base de datos**: la carpeta contiene archivos MXF que la base de datos no conoce. Puede que Media Composer no los muestre.
- **Entradas de la base de datos sin archivo**: la base de datos nombra archivos que no están en la carpeta. Aparecen como medios sin conexión.
- **Sin base de datos**: la carpeta tiene archivos MXF pero ningún archivo de base de datos.
- **Falta msmFMID.pmr o msmMMOB.mdb**: falta uno de los dos archivos de base de datos.
- **Base de datos vacía**: `msmFMID.pmr` no tiene contenido.
- **Base de datos ilegible**: el archivo no se pudo leer o tiene un formato que MediaFiles Check no conoce. Nunca adivina.

Se muestran hasta 10 nombres de archivo por hallazgo. **Guardar informe…** escribe todas las diferencias con cada nombre de archivo en un archivo de texto.

## Cambiar el nombre para Media Composer

Media Composer solo reconstruye las bases de datos de sus propias carpetas: carpetas numeradas como `1` o `2` en discos locales, `<nombre del ordenador>.<número>` (p. ej. `Mac Studio.3`) en discos de red. Las carpetas con otros nombres, p. ej. copiadas de otro sistema, conservan su base de datos antigua.

Después de una comprobación, **Cambiar el nombre para Media Composer…** ofrece cambiar el nombre de esas carpetas:

1. Cierra Media Composer y haz clic en **Cambiar el nombre**. Cada carpeta recibe el siguiente número libre (o nombre del ordenador y número). MediaFiles Check recuerda el nombre original y cada archivo de medios.
2. Haz clic en **Abrir Media Composer** (o ábrelo tú) y espera a que haya analizado los discos. En la página de inicio, el punto de una carpeta se vuelve verde en cuanto Media Composer ha escrito su base de datos.
3. Cierra Media Composer y haz clic en **Recuperar el nombre**.

Antes de recuperar el nombre, MediaFiles Check comprueba cada carpeta:

- **Archivos nuevos**: normalmente medios que Media Composer grabó en la carpeta mientras tenía el nombre nuevo. Puedes elegir: moverlos a una carpeta numerada nueva, moverlos a la carpeta `1` (o `<nombre del ordenador>.1`) o dejarlos en la carpeta.
- **Archivos que faltan o han cambiado**: tú decides si se recupera el nombre de todos modos.
- **Base de datos aún no reescrita**: tú decides si se recupera el nombre de todos modos.

**Dejarla así por ahora** deja la carpeta como está; puedes recuperar el nombre más tarde. Si mientras tanto existe una carpeta con el nombre original, no se cambia nada y se te avisa.

## Proyectos de Avid

**Abrir proyectos…** en la página de inicio abre la ventana de proyectos de Avid. Encuentra los archivos de medios que necesitan los bins de tus proyectos y que no están en ningún disco conectado, y escribe una solicitud para MediaFiles Ferry, que los reúne en el sistema que los tiene.

1. **Elegir carpeta…**: la carpeta con tus proyectos de Avid. Cada carpeta con un archivo `.avp` cuenta como proyecto (las copias de seguridad en `Attic` se omiten). La carpeta se recuerda.
2. Activa los proyectos que quieres comprobar; al principio todos están desactivados.
3. Haz clic en **Analizar**. MediaFiles Check lee cada bin y sigue cada master clip, subclip y secuencia hasta los archivos de medios que usan, también entre bins del proyecto. Los compara con las bases de datos de Avid de todos los discos conectados.
4. El resultado muestra, por bin, cada archivo de medios que falta una sola vez, con el nombre de su master clip y «3 archivos» cuando faltan p. ej. la imagen y el sonido de un clip. Haz clic en un archivo para ver dónde se usa en el bin (master clip, subclips, secuencias). **Guardar lista…** escribe la lista completa en un archivo de texto.
5. **Guardar solicitud…** escribe un archivo `…_Request.ferry`. Ábrelo en MediaFiles Ferry en el sistema que tiene los medios (**Abrir manifiesto…**); Ferry reúne los archivos en una entrega normal, que luego importas aquí con **Comprobar e importar**.

Conviene saber:

- Clips y medios se asocian por los identificadores de medios de Avid, no por los nombres de archivo. Un archivo de un disco que no está en **ninguna** base de datos de Avid se identifica por el identificador de medios de su cabecera MXF; solo los archivos con cabecera ilegible (dañados, aún escribiéndose) quedan sin reconocer: el resultado lo advierte.
- Los archivos vinculados por AMA se omiten (solo se cuentan).
- Un archivo que falta cuya fuente (clip de cámara o cinta, imagen o sonido) está en los discos como otro archivo de medios se marca **en los discos – revincular**: el clip solo perdió su vínculo; revincúlalo en Media Composer. No se solicita.
- Los archivos sin referencia de medios de Avid (p. ej. importados por AAF) se cuentan aparte. Ferry solo los encuentra si el otro sistema los tiene en Avid MediaFiles.
- Los archivos de render que faltan (efectos renderizados) solo se cuentan, no se listan ni se solicitan: Media Composer los vuelve a renderizar.
- Un disco que no responde durante 30 segundos se omite y se nombra en el resultado.

## Reconstruir una base de datos

1. Cierra Media Composer.
2. Mueve `msmMMOB.mdb` y `msmFMID.pmr` fuera de la carpeta, p. ej. a una carpeta en el escritorio. No los borres hasta que todo vuelva a funcionar.
3. Abre Media Composer. Escribe una base de datos nueva para la carpeta.
4. Comprueba la carpeta de nuevo.

## Ajustes

**MediaFiles Check → Ajustes…** (⌘,) ajusta el aspecto (claro u oscuro) y el idioma (English, Deutsch, Français, Español). **MediaFiles Check → Acerca de MediaFiles Check** muestra la versión y la licencia.

## Actualizaciones

MediaFiles Check busca una nueva versión en GitHub poco después de arrancar y cada 6 horas; **Ajustes… → Buscar actualizaciones** lo hace al momento. Una nueva versión aparece arriba en la ventana principal: **Novedades** abre las notas de la versión, **Instalar actualización** la descarga, comprueba que esté completa e intacta, mueve la versión anterior a la papelera y reinicia la app.

## Archivo de registro

MediaFiles Check anota lo que hace (comprobaciones, cambios de nombre, errores) en un archivo de registro. **Ayuda → Mostrar archivo de registro** lo muestra en el Finder; envíalo cuando informes de un problema.
