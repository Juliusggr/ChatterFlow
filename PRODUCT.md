# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Tauri v2, React, TypeScript, Tailwind CSS, Lucide, SQLite via rusqlite. Confirmado por el brief del proyecto. La interfaz web corre dentro de la ventana de escritorio.

## Users

Administradores y chatters de una agencia de OnlyFans. Registran empleados, asignan roles y horarios, y abren o cierran turnos mientras trabajan las cuentas de las modelos.

## Product Purpose

CreatorFloww es el CRM de escritorio de la agencia. El éxito de esta fase es que el equipo real se lee y se guarda en SQLite, y que el escritorio muestra las modelos que de verdad están en la base.

## Positioning

Cada modelo, empleado, turno y venta vive en la base local. Las ventas de OnlyFans entran con el id real de la plataforma para no duplicarse. Esta fase no sincroniza OnlyFans; solo deja el escritorio y el módulo de equipos.

## Operating Context

Aplicación de escritorio. La base `creatorfloww.db` está en el directorio de datos de la app. Los turnos se abren con Clock in y se cierran con Clock out. Las secciones pedidas son Dashboard, Equipos y Roles, Modelos, Messages y Analytics.

## Capabilities and Constraints

- Roles válidos: Owner, Admin, Chatter, Dropper.
- Prohibido inventar empleados, modelos, saldos o métricas. Si SQLite está vacío, la interfaz muestra estados vacíos reales.
- Sin proxies y sin adelantar la ventana de OnlyFans, los gráficos ni la exportación.
- La imagen de referencia de esta fase no llegó adjunta. La composición sigue el texto de la fase y las reglas visuales ya fijadas: tarjetas blancas, borde slate, sombra suave, acentos azul y naranja.

## Brand Commitments

Nombre: CreatorFloww. Estilo de interfaz fijado por el proyecto: CRM tipo Infloww, minimalista, tarjetas blancas con `border-slate-200` y `shadow-sm`, acentos azul y naranja.

## Evidence on Hand

No hay cuentas, empleados ni capturas de producción en el repositorio. La interfaz no debe fabricar rostros, nombres ni cifras.

## Product Principles

- La base local es la única fuente de lo que se muestra.
- Un turno abierto por empleado; cerrarlo no borra el registro.
- Los estados vacíos se dicen en voz alta, no se rellenan.
- El escritorio se recorre por el carril de modelos y la navegación, sin módulos que esta fase no pidió.
