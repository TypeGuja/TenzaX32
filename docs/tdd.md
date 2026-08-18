# Pony Animation Studio — Technical Design Document

**Версия:** 1.1 (дополнено разделами 91–103)
**Тип приложения:** 2D vector animation editor
**Основная задача:** создание, редактирование, анимация и экспорт мультфильмов на основе векторных персонажей и SVG-ассетов.

---

# 1. Назначение программы

Программа представляет собой полноценный 2D-векторный редактор и анимационную среду, предназначенную прежде всего для создания мультфильмов с персонажами, состоящими из отдельных векторных частей.

Основные возможности:

* рисование векторной графики;
* создание и редактирование SVG;
* импорт существующих SVG;
* экспорт SVG;
* работа с растровыми изображениями;
* создание персонажей из отдельных частей;
* создание иерархий объектов;
* кости и риггинг;
* покадровая анимация;
* tween-анимация;
* трансформация объектов;
* анимация параметров SVG;
* работа со слоями;
* символы/компоненты;
* маски и clipping;
* фильтры;
* управление порядком отрисовки;
* монтаж сцен;
* экспорт готовой анимации;
* проектная система с несколькими сценами.

Программа **не должна быть копией Adobe Animate**. Интерфейс может быть похож по логике на профессиональные 2D-редакторы, однако внутренняя модель данных должна быть современной и ориентированной на SVG и полноценную векторную анимацию.

---

# 2. Основная архитектура

Программа состоит из следующих подсистем:

```text
Application
│
├── Project Manager
│
├── Document System
│
├── SVG Engine
│   ├── Parser
│   ├── DOM
│   ├── Geometry
│   ├── Styles
│   ├── Transforms
│   ├── Gradients
│   ├── Masks
│   ├── Filters
│   └── Exporter
│
├── Drawing Engine
│   ├── Selection
│   ├── Pen
│   ├── Pencil
│   ├── Shapes
│   ├── Node Editor
│   └── Boolean Geometry
│
├── Scene Graph
│
├── Layer System
│
├── Animation System
│   ├── Timeline
│   ├── Keyframes
│   ├── Curves
│   ├── Tweening
│   └── Animation Clips
│
├── Rigging System
│   ├── Bones
│   ├── Constraints
│   ├── IK
│   └── Skinning
│
├── Rendering
│
├── Asset Manager
│
├── File System
│
├── Undo/Redo
│
├── Plugin System
│
├── Debugger
│
└── Export Pipeline
```

---

# 3. Основная модель документа

Документ состоит из:

```text
Project
│
├── Project Settings
├── Assets
│
├── Scenes
│   │
│   ├── Scene
│   │   ├── Layers
│   │   │   ├── Objects
│   │   │   ├── Groups
│   │   │   └── Symbols
│   │   │
│   │   └── Timeline
│   │
│   └── ...
│
└── Global Resources
```

Каждый объект должен иметь уникальный внутренний ID.

Например:

```text
object_id = "obj_8f31..."
```

Имя объекта не должно использоваться как уникальный идентификатор.

---

# 4. Scene Graph

Все визуальные объекты находятся в Scene Graph.

Пример:

```text
Scene
└── Pony
    ├── Body
    ├── Head
    │   ├── Eye_L
    │   ├── Eye_R
    │   └── Ear
    ├── Mane
    ├── Tail
    └── CutieMark
```

Каждый объект может иметь:

* parent;
* children;
* local transform;
* world transform;
* visibility;
* opacity;
* blend mode;
* z-order;
* style;
* animation tracks;
* metadata.

---

# 5. Векторная модель

Основным графическим примитивом является SVG-compatible vector object.

Поддерживаемые базовые элементы:

* `rect`;
* `circle`;
* `ellipse`;
* `line`;
* `polyline`;
* `polygon`;
* `path`;
* `image`;
* `text` — архитектурно поддерживается, но интерфейс текстового инструмента может быть отключён;
* `g`;
* `symbol`;
* `use`;
* `clipPath`;
* `mask`;
* `defs`.

SVG является XML-языком описания 2D-векторной и смешанной vector/raster графики. В SVG группы `g` используются как контейнеры для связанных объектов, а `id` может использоваться для идентификации и повторного использования объектов.

---

# 6. SVG Document

Минимальный SVG:

```xml
<svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 1920 1080">

    <rect
        id="background"
        x="0"
        y="0"
        width="1920"
        height="1080"
        fill="#000000"/>

</svg>
```

Программа должна сохранять:

* namespace;
* viewBox;
* width;
* height;
* элементы;
* порядок элементов;
* группы;
* transforms;
* styles;
* gradients;
* masks;
* clipping;
* metadata.

---

# 7. viewBox

`viewBox` определяет внутреннюю систему координат SVG.

Например:

```xml
viewBox="0 0 1920 1080"
```

означает:

```text
left   = 0
top    = 0
width  = 1920
height = 1080
```

`viewBox` совместно с `preserveAspectRatio` определяет способ масштабирования SVG viewport.

Редактор должен позволять изменять:

* размер canvas;
* viewBox;
* aspect ratio;
* масштаб отображения.

---

# 8. Path

`path` является главным инструментом сложного векторного рисования.

Поддерживаются команды:

```text
M — Move
L — Line
H — Horizontal
V — Vertical
C — Cubic Bézier
S — Smooth cubic
Q — Quadratic Bézier
T — Smooth quadratic
A — Arc
Z — Close
```

Пример:

```xml
<path
    d="M 10 10
       C 50 0 100 0 150 50
       C 100 100 50 100 10 10
       Z"
    fill="#608BFF"/>
```

Редактор должен хранить path как структурированную геометрию, а не только как строку `d`.

---

# 9. Узлы Path

Каждая path может редактироваться через Node Tool.

Типы узлов:

* corner;
* smooth;
* symmetric;
* auto-smooth.

Каждый Bézier node должен хранить:

```text
position
in_handle
out_handle
node_type
```

Редактор должен поддерживать:

* добавление узла;
* удаление узла;
* перемещение;
* изменение handle;
* преобразование corner ↔ smooth;
* разрыв path;
* объединение path;
* закрытие path;
* изменение направления path.

---

# 10. Инструменты панели

Основная панель инструментов:

```text
Selection
Node
Transform
Free Transform
Pen
Pencil
Brush
Eraser
Rectangle
Ellipse
Polygon
Line
Paint Bucket
Eyedropper
Gradient
Hand
Zoom
Bone
Rig
Pivot
```

---

# 11. Selection Tool

Назначение:

Выбор объектов.

Возможности:

* выбор одного объекта;
* multi-selection;
* drag selection;
* добавление/удаление из selection;
* выбор группы;
* выбор дочернего объекта;
* выбор по слою.

Горячая клавиша:

```text
V
```

---

# 12. Node Tool

Редактирование геометрии.

Горячая клавиша:

```text
A
```

При выборе path отображаются:

```text
●───────●
 \     /
  \   /
   \ /
```

Пользователь может изменять:

* координаты узлов;
* tangent handles;
* тип узлов;
* segment type.

---

# 13. Transform Tool

Позволяет изменять:

* position;
* rotation;
* scale;
* skew;
* pivot.

Преобразования должны храниться как transform matrix либо как нормализованный набор параметров.

Рекомендуемый внутренний порядок:

```text
Translate
→ Rotate
→ Scale
→ Skew
```

---

# 14. Pivot Tool

Pivot является центром трансформации объекта.

Он должен быть отдельным редактируемым параметром:

```text
pivot_x
pivot_y
```

Это особенно важно для персонажей.

Например:

```text
Body
 └── FrontLeg
       pivot = shoulder
```

Тогда вращение ноги происходит вокруг плеча, а не вокруг центра объекта.

---

# 15. Pen Tool

Создание Bézier paths.

Поддерживает:

* click → line;
* click-drag → Bézier;
* closed paths;
* open paths;
* продолжение существующего path.

Опции:

```text
Fill
Stroke
Stroke Width
Cap
Join
```

---

# 16. Pencil Tool

Свободное рисование.

Input:

```text
mouse
tablet
pen pressure
```

Должна существовать система simplification.

Параметры:

```text
Smoothing
Accuracy
Minimum Length
Pressure
```

Нарисованный stroke автоматически преобразуется в оптимизированный Bézier path.

---

# 17. Brush Tool

В отличие от Pencil Tool, Brush должен поддерживать:

* variable width;
* pressure;
* taper;
* roundness;
* spacing;
* opacity;
* custom brush profile.

Brush должен сохраняться как vector geometry.

---

# 18. Eraser Tool

Удаление частей vector geometry.

Режимы:

```text
Object Eraser
Path Eraser
Point Eraser
Stroke Eraser
```

При пересечении path редактор должен уметь выполнять геометрическое разделение.

---

# 19. Shape Tools

Rectangle:

```text
x
y
width
height
radius
fill
stroke
```

Ellipse:

```text
cx
cy
rx
ry
```

Polygon:

```text
center
radius
sides
rotation
```

Line:

```text
x1
y1
x2
y2
```

Все фигуры должны оставаться редактируемыми до момента явного преобразования в path.

---

# 20. Fill

Каждый объект может иметь:

```text
None
Solid Color
Linear Gradient
Radial Gradient
Pattern
```

Цвет должен поддерживать:

```text
RGB
RGBA
HEX
HSL
```

---

# 21. Stroke

Stroke содержит:

```text
color
width
opacity
linecap
linejoin
miterlimit
dasharray
dashoffset
```

Cap:

```text
Butt
Round
Square
```

Join:

```text
Miter
Round
Bevel
```

---

# 22. Gradient Tool

Linear:

```text
start
end
stops[]
```

Radial:

```text
center
radius
focal point
stops[]
```

Каждый stop:

```text
offset
color
opacity
```

> **Статус реализации:** Linear (`start`/`end`/`stops[]`) и Radial
> (`center`/`radius`/`stops[]`) реализованы — см. раздел 60 ниже
> (`GradientDef`/`GradientKind`) и README "Что уже работает" для
> деталей data-модели, SVG round-trip и GUI-панели **Gradients**.
> `offset`/`color`/`opacity` на каждом stop — есть (`opacity` хранится
> слитно с цветом через альфа-канал `RgbaColor.a`, не отдельным полем —
> тот же приём, что и у обычной заливки фигур). Не реализовано: `focal
> point` у Radial (SVG `fx`/`fy`, смещение фокуса относительно центра)
> — упрощение до окружности с одним центром, покрывает подавляющее
> большинство практических случаев (свечение, объём); полная версия —
> отдельная задача при первом реальном запросе на неё.

---

# 23. Eyedropper

Получает:

* fill;
* stroke;
* opacity;
* gradient;
* style.

Режимы:

```text
Sample Object
Sample Appearance
Sample Canvas
```

---

# 24. Paint Bucket

Paint Bucket должен создавать заполненную область на основе замкнутых контуров.

Параметры:

```text
Gap Tolerance
Merge Regions
Create New Object
Fill Selected Object
```

---

# 25. Layers

Каждый слой имеет:

```text
id
name
visible
locked
opacity
blend_mode
children
timeline
```

Типы:

```text
Vector Layer
Raster Layer
Guide Layer
Camera Layer
Bone Layer
Folder Layer
```

---

# 26. Layer hierarchy

Пример:

```text
Pony
├── Body
├── Head
├── Mane
│   ├── Mane_Back
│   └── Mane_Front
├── Legs
│   ├── Front_L
│   ├── Front_R
│   ├── Back_L
│   └── Back_R
└── Tail
```

Folder layers позволяют организовывать сложных персонажей.

---

# 27. Groups

SVG `<g>` должен соответствовать группе Scene Graph.

Группа может содержать:

* paths;
* shapes;
* nested groups;
* symbols;
* images.

SVG `g` является контейнером и может содержать другие `g` на произвольной глубине.

> **Статус реализации:** реализован как организационная структура ПОВЕРХ
> `Character::parts` (`pony_core::group::{GroupTree, PartGroup}`,
> `Character::groups`), намеренно отдельно от `Skeleton`/`Bone`-иерархии,
> которая управляет деформацией. Вложенность произвольной глубины,
> reparent с защитой от циклов, скрытие с распространением на весь
> вложенный поддерево, Ungroup освобождает содержимое не удаляя его —
> см. README, раздел "Что уже работает". "Может содержать paths/shapes"
> в этом разделе относится к SVG-документу (`VectorDoc`) — там группировка
> реализована на уровне парсера как flatten с накоплением transform (см.
> `collect_shapes` в `vector.rs`, раздел 27 уже упоминается в её
> комментариях), не как сохраняемая структура (`VectorDoc` — плоская
> модель по дизайну, см. раздел 29). `Character::groups` — ОТДЕЛЬНАЯ
> группировка, на уровне частей персонажа, не фигур одного SVG-документа.

---

# 28. Symbols

Symbol — reusable object.

Например:

```text
Eye
```

может использоваться:

```text
Eye_L
Eye_R
```

Обе копии ссылаются на один Symbol Definition.

Это уменьшает размер документа и позволяет изменять исходный объект.

> **Статус реализации:** реализован (`pony_core::vector::SymbolDef` +
> `VectorShape::Instance`, поле `VectorDoc::symbols`). `resolve_symbol_
> instance()` резолвит инстанс из АКТУАЛЬНОГО определения каждый раз —
> правка `SymbolDef` реально видна на всех инстансах сразу, включая
> вложенные (символ, ссылающийся на другой символ). Eye -> Eye_L/Eye_R
> пример выше реализован буквально через отрицательный `scale_x`
> (`transform.0 < 0`) одного из двух инстансов одного `SymbolDef`.
> Сериализуется настоящим SVG `<symbol>`/`<use>` (не инлайн-копированием),
> `resvg` понимает нативно. Override для инстансов (`fill_override`,
> раздел 95) и "Break Apart Symbol" (`VectorDoc::break_apart_symbol_
> instance`) — тоже реализованы, см. раздел 95 ниже и README, раздел "Что
> уже работает". В GUI — вкладка Symbols (Convert to Symbol / список
> определений с плейсментом инстансов / Break Apart).

---

# 29. SVG import

При импорте SVG:

```text
SVG file
 ↓
XML Parser
 ↓
SVG DOM
 ↓
Validation
 ↓
Normalization
 ↓
Internal Scene Graph
 ↓
Renderer
```

Импортёр должен поддерживать:

* `svg`;
* `g`;
* `path`;
* `rect`;
* `circle`;
* `ellipse`;
* `line`;
* `polyline`;
* `polygon`;
* gradients;
* transforms;
* clipPath;
* mask;
* image;
* style;
* metadata.

Неизвестные элементы не должны немедленно уничтожаться.

Они должны помещаться в:

```text
Unsupported SVG Data
```

для возможного round-trip сохранения.

---

# 30. SVG export

Экспорт должен иметь режимы:

```text
Optimized SVG
Editable SVG
Compatibility SVG
Flattened SVG
```

Optimized:

* удаляет ненужные metadata;
* объединяет стили;
* оптимизирует paths;
* удаляет невидимые элементы.

Editable:

* сохраняет максимальную структуру редактора.

Flattened:

* преобразует сложные объекты в максимально простые SVG primitives.

---

# 31. SVG round-trip

Ключевое требование:

```text
Import SVG
→ Edit
→ Export SVG
```

не должен без причины уничтожать структуру документа.

Например:

```xml
<g id="head">
    <path id="ear"/>
    <path id="eye"/>
</g>
```

после загрузки и сохранения должен оставаться логически эквивалентным.

---

# 32. SVG и анимация

Анимация не должна разрушать базовую SVG geometry.

Объект:

```text
Mane
```

остаётся одним объектом.

Анимация хранится отдельно:

```text
Mane
│
├── Geometry
├── Style
└── Animation
     ├── Position
     ├── Rotation
     ├── Scale
     └── Deformation
```

Это критически важно для персонажей.

---

# 33. Timeline

Timeline состоит из:

```text
Frame
Keyframe
Track
Layer
```

Пример:

```text
0       12       24       36
│--------│--------│--------│
●                 ●
```

Keyframe содержит состояние параметра.

---

# 34. Animation Track

Каждый animatable property может иметь отдельный track:

```text
Position X
Position Y
Rotation
Scale X
Scale Y
Opacity
Stroke Width
Fill Color
Path Geometry
Bone Transform
```

---

# 35. Keyframes

Keyframe содержит:

```text
time
value
interpolation
easing
```

Типы interpolation:

```text
Step
Linear
Bezier
Ease In
Ease Out
Ease In Out
Custom
```

---

# 36. Tween

Tween автоматически интерполирует состояние между двумя keyframes.

Пример:

```text
Frame 0
rotation = 0°

       ↓ tween

Frame 30
rotation = 90°
```

---

# 37. Motion curves

Редактор должен позволять редактировать кривые:

```text
Value
 ^
 |       ╭────
 |     ╭─
 |   ╭─
 | ╭─
 +────────────────> Time
```

Каждая кривая должна иметь control points.

---

# 38. Frame-by-frame animation

Для покадровой анимации каждый frame может содержать собственное состояние рисунка.

Режим:

```text
Frame 1 → Drawing 1
Frame 2 → Drawing 2
Frame 3 → Drawing 3
```

Должна существовать функция Onion Skin.

---

# 39. Onion Skin

Режимы:

```text
Previous
Next
Previous + Next
Range
```

Предыдущие кадры отображаются с уменьшенной opacity.

---

# 40. Character Rig

Персонаж может быть представлен:

```text
Skeleton
│
├── Root
├── Body
├── Neck
├── Head
├── FrontLeg_L
├── FrontLeg_R
├── BackLeg_L
├── BackLeg_R
├── Mane
└── Tail
```

Каждая кость:

```text
position
rotation
length
parent
constraints
```

---

# 41. IK

Inverse Kinematics должна поддерживать:

```text
Two Bone IK
Chain IK
Target
Pole Target
```

Например:

```text
Hip
 │
Knee
 │
Hoof ← Target
```

Перемещение копыта автоматически изменяет положение колена и бедра.

> **Статус реализации:** Two Bone IK реализован (`pony_core::ik::solve_two_bone_ik`,
> аналитическое решение законом косинусов, с рабочим Pole Target) и подключён
> к рендеру (`Skeleton::world_transform_with_ik`) и к GUI (вкладка Bones —
> секция "IK (Two Bone)"). Chain IK (цепочка произвольной длины) пока не
> реализован — two-bone покрывает основной практический случай (нога/рука)
> без итеративного солвера, который потребовался бы для цепочки произвольной
> длины. См. README, раздел "Что уже работает" — там же описаны найденные
> при разработке баги (направление кости в rest-позе, длина звена).

---

# 42. Deformation

Для мягких частей персонажа должна существовать возможность деформации:

```text
Mane
Tail
Ears
Cheeks
Body
```

Возможные системы:

```text
Mesh Deformation
Bezier Deformation
Lattice
Bone Deformation
```

---

# 43. Camera

Scene может иметь Camera Layer.

Параметры:

```text
Position
Zoom
Rotation
Shake
Perspective Simulation
```

Camera также должна быть animatable.

---

# 44. File menu

## New

Создаёт новый проект.

Параметры:

```text
Width
Height
FPS
Background
Color Space
Resolution
```

## Open

Открывает:

```text
Project
SVG
Image
Animation
```

## Save

Сохраняет текущий проект.

## Save As

Создаёт новую копию проекта.

## Import

Импортирует asset.

## Export

Экспортирует:

```text
SVG
PNG
JPEG
WebP
GIF
MP4
Image Sequence
```

## Recent Files

Список последних файлов.

## Project Properties

Настройки проекта.

## Exit

Закрывает приложение.

---

# 45. Edit menu

## Undo

Отмена последней операции.

## Redo

Повтор операции.

## Cut

Вырезать.

## Copy

Копировать.

## Paste

Вставить.

## Paste in Place

Вставить на исходную позицию.

## Duplicate

Создать копию.

## Delete

Удалить.

## Select All

Выбрать всё.

## Deselect

Снять выделение.

## Preferences

Настройки программы.

Undo/Redo должны работать через Command Pattern:

```text
Command
├── execute()
└── undo()
```

---

# 46. View menu

## Zoom In

Увеличение.

## Zoom Out

Уменьшение.

## Fit Canvas

Подогнать canvas под окно.

## Actual Size

100%.

## Show Grid

Сетка.

## Snap to Grid

Привязка.

## Show Guides

Направляющие.

## Show Rulers

Линейки.

## Show Onion Skin

Onion Skin.

## Show Outlines

Только контуры.

## Fullscreen

Полноэкранный режим.

---

# 47. Insert menu

## New Layer

Новый слой.

## New Folder

Новая папка.

## New Symbol

Новый symbol.

## New Scene

Новая сцена.

## Keyframe

Создать keyframe.

## Blank Keyframe

Создать пустой keyframe.

## Bone

Добавить bone.

## Camera

Добавить camera.

## Asset

Добавить внешний asset.

---

# 48. Modify menu

## Transform

* Move
* Rotate
* Scale
* Skew
* Free Transform

## Arrange

```text
Bring to Front
Bring Forward
Send Backward
Send to Back
```

## Group

Объединить объекты.

## Ungroup

Разгруппировать.

## Convert to Path

Преобразовать в path.

## Convert to Symbol

Создать symbol.

## Combine Paths

Boolean operations:

```text
Union
Difference
Intersection
XOR
Divide
```

> **Статус реализации:** все пять операций реализованы —
> `pony_core::boolean::{BooleanOp, boolean_op}`, через настоящий
> проверенный сканлайн-движок полигонального оверлея (`i_overlay`), не
> собственная наивная реализация. Фигуры сначала флэттенятся в полигон
> (кривые Безье честно семплируются, не аппроксимируются грубо), а не
> только Rect/Ellipse — см. README, "Что уже работает", для деталей
> (включая честное ограничение — дырки в результате не хранятся, т.к.
> `VectorShape::Polygon` не поддерживает составные контуры, но это
> явно репортится, не теряется молча) и `pony_core::boolean::tests` для
> проверки (15 тестов). В GUI — вкладка **Combine Paths**.

## Align

```text
Left
Center
Right
Top
Middle
Bottom
```

## Distribute

Равномерное распределение объектов.

---

# 49. Text menu

Текстовый интерфейс не является приоритетом.

Архитектура должна оставлять возможность добавить text object позднее.

---

# 50. Commands menu

Commands предназначен для пользовательских автоматизаций.

Примеры:

```text
Optimize SVG
Remove Invisible Objects
Convert All Strokes
Normalize Transforms
Generate LOD
Rename Selected
Create Character Rig
```

Пользовательские команды могут быть скриптами.

---

# 51. Control menu

Управление воспроизведением.

```text
Play
Pause
Stop
Loop
Step Forward
Step Backward
Go to First Frame
Go to Last Frame
```

Горячие клавиши:

```text
Space = Play/Pause
Home = First Frame
End = Last Frame
Left = Previous Frame
Right = Next Frame
```

---

# 52. Debug menu

Предназначен для разработчика.

## Debug Renderer

Показывает:

```text
Draw Calls
Vertices
Paths
Textures
GPU Memory
CPU Time
Frame Time
```

## Scene Graph Debugger

Показывает дерево объектов.

## SVG Inspector

Показывает реальное внутреннее SVG-представление.

## Animation Debugger

Показывает:

```text
Active Tracks
Keyframes
Interpolation
Current Values
```

## Memory Statistics

```text
RAM
GPU VRAM
Asset Memory
Geometry Memory
Cache
```

## Performance Profiler

```text
Frame Time
Render Time
Update Time
Animation Time
Physics Time
IO Time
```

---

# 53. Window menu

Окна программы:

```text
Timeline
Layers
Properties
Tools
Color
Assets
Library
Scene
Console
Debugger
SVG Inspector
Animation Graph
Character Rig
```

Панели должны быть dockable.

Пользователь может:

* перемещать;
* закреплять;
* откреплять;
* изменять размер;
* закрывать.

---

# 54. Help menu

## Documentation

Открывает документацию.

## Keyboard Shortcuts

Список горячих клавиш.

## SVG Reference

Справочник SVG.

## Tutorials

Обучающие материалы.

## Check for Updates

Проверка обновлений.

## About

Информация о программе.

---

# 55. Properties panel

При выборе объекта показывает:

```text
Transform
├── X
├── Y
├── Rotation
├── Scale X
├── Scale Y
├── Skew X
└── Skew Y

Appearance
├── Fill
├── Stroke
├── Opacity
└── Blend Mode

Geometry
├── Width
├── Height
└── Bounds

Animation
├── Keyframes
└── Tracks
```

---

# 56. Asset Library

Все импортированные ресурсы:

```text
Characters
Backgrounds
Props
Audio
Images
SVG
Symbols
Animations
```

Каждый asset имеет:

```text
id
name
type
path
metadata
dependencies
```

---

# 57. Project format

Рекомендуется использовать собственный проектный формат.

Например:

```text
pony_project/
│
├── project.json
├── scenes/
│   ├── scene_001.json
│   └── scene_002.json
│
├── assets/
│   ├── pony.svg
│   ├── background.svg
│   └── textures/
│
├── audio/
│
└── cache/
```

Не рекомендуется хранить весь проект в одном гигантском бинарном файле.

Adobe также использует разделение между рабочим FLA и распакованным XFL-представлением, где проект может быть представлен набором файлов; подобная модель удобна для совместной работы и контроля отдельных компонентов.

---

# 58. Autosave

Автосохранение:

```text
каждые 30 секунд
```

или после значительного количества изменений.

Создавать:

```text
project.autosave
```

При аварийном завершении:

```text
Recover previous session?
```

---

# 59. Undo/Redo

Undo должен поддерживать:

* drawing;
* transformations;
* layer operations;
* SVG editing;
* animation;
* rigging;
* imports;
* deletes.

Для больших операций желательно использовать snapshots + command log.

---

# 60. Rendering

Renderer должен поддерживать:

```text
Vector Rasterization
Anti-Aliasing
Alpha Blending
Gradients
Masks
Clipping
Filters
Transforms
```

Архитектура:

```text
Scene Graph
 ↓
Culling
 ↓
Geometry Processing
 ↓
Tessellation
 ↓
Batching
 ↓
GPU Renderer
 ↓
Framebuffer
```

> **Статус реализации:** из этого списка реализованы Vector Rasterization,
> Anti-Aliasing (MSAA), Alpha Blending, Transforms, **Masks/Clipping**
> (`Part::clip_by`, `Character::resolve_clip_mask`, GPU-семплинг второй
> текстуры через обратную матрицу модели маски — см. README, раздел
> "Что уже работает", и `pony-render::renderer::mask_gpu_tests` для
> проверки настоящим headless GPU-рендером) и **Gradients**
> (`pony_core::vector::{GradientDef, GradientKind, GradientStop}`,
> аддитивное поле `fill_gradient: Option<String>` на `Rect`/`Ellipse`/
> `Polygon`/`Path`, честный SVG round-trip с `<linearGradient>`/
> `<radialGradient>`, настоящий per-vertex рендер в GUI-превью и
> реальная resvg-растеризация в SVG-экспорте — см. README, раздел "Что
> уже работает", и `pony_core::vector::tests::gradient_*` /
> `pony-render::texture::vector_roundtrip_tests::drawn_gradient_ellipse_*`
> для проверки). Masks реализованы как растровая альфа-маска (одна
> часть маскирует другую своей альфой) — ближе к маскирующему слою
> Adobe Animate/Photoshop, чем к SVG `clipPath`, т.к. раздел явно
> перечисляет Masks РЯДОМ с Alpha Blending в одном списке растровых
> техник, а не как геометрический clip-path. Filters — НЕ реализованы
> (см. README, "Крупнейшие настоящие пробелы"). Архитектура выше
> (Scene Graph -> Culling -> ... -> Framebuffer) описана в общих
> чертах, не как отдельный формализованный пайплайн-модуль — рендер в
> этом движке линейный (`Renderer::render_character` строит bind group
> на часть и рисует её), явного Culling/Batching прохода нет (раздел 61
> ниже — тоже не реализован).

---

# 61. Culling

Объекты, находящиеся вне viewport, не должны отрисовываться.

Минимально:

```text
AABB Frustum Culling
```

Позднее:

```text
Quadtree
BVH
```

---

# 62. SVG caching

Статические SVG objects должны кэшироваться.

Если объект не изменился:

```text
Geometry Cache
     ↓
reuse
```

Не следует каждый frame заново парсить SVG/XML.

---

# 63. Animation evaluation

Animation system не должен изменять оригинальные SVG-файлы.

Pipeline:

```text
Base State
   +
Animation State
   ↓
Evaluated State
   ↓
Renderer
```

Это позволяет:

* проигрывать;
* перематывать;
* отменять;
* менять animation speed.

---

# 64. Audio

Scene может содержать:

```text
Music Track
Dialogue Track
SFX Track
```

Audio должен иметь:

```text
volume
pan
start
end
fade
```

---

# 65. Export animation

Pipeline:

```text
Scene
 ↓
Timeline Evaluation
 ↓
Frame Renderer
 ↓
Frame Buffer
 ↓
Encoder
```

Поддерживаемые форматы:

```text
PNG sequence
WebP sequence
GIF
MP4
WebM
```

Для SVG-анимации:

```text
SVG Animation
```

если используемые элементы поддерживаются целевым форматом.

---

# 66. SVG animation export

Для SVG должен существовать отдельный режим:

```text
Static SVG
Animated SVG
```

Animated SVG может использовать:

* SMIL-compatible animation;
* CSS animation;
* JS animation.

Однако внутренний animation model программы не должен зависеть от одного конкретного экспортного механизма.

---

# 67. Hotkeys

Основные:

```text
V       Selection
A       Node
P       Pen
B       Brush
N       Pencil
E       Eraser
R       Rectangle
O       Ellipse
G       Gradient
I       Eyedropper
H       Hand
Z       Zoom

Ctrl+Z  Undo
Ctrl+Y  Redo
Ctrl+C  Copy
Ctrl+V  Paste
Ctrl+X  Cut
Ctrl+D  Duplicate
Ctrl+S  Save
Ctrl+O  Open
Ctrl+N  New

Space   Play/Pause
Home    First Frame
End     Last Frame
```

---

# 68. Snapping

Поддержка:

```text
Grid
Pixel
Object
Node
Guide
Center
Angle
```

Например:

```text
Snap Rotation = 15°
```

---

# 69. Guides

Направляющие:

```text
Horizontal
Vertical
Diagonal
Custom
```

Объекты могут привязываться к guides.

---

# 70. Onion Skin architecture

Onion Skin не должен создавать реальные дополнительные объекты.

Renderer получает:

```text
Frame N-3
Frame N-2
Frame N-1
Current
Frame N+1
Frame N+2
```

и визуализирует их с различными alpha.

---

# 71. Character workflow

Типичный workflow:

```text
1. Создать проект
2. Создать сцену
3. Импортировать/нарисовать SVG
4. Разделить персонажа на части
5. Создать hierarchy
6. Установить pivots
7. Создать skeleton
8. Привязать части
9. Создать animation
10. Добавить keyframes
11. Настроить curves
12. Добавить camera
13. Добавить audio
14. Preview
15. Export
```

---

# 72. Pony character structure

Рекомендуемая структура:

```text
Pony
│
├── Body
│
├── Head
│   ├── Ear_L
│   ├── Ear_R
│   ├── Eye_L
│   ├── Eye_R
│   ├── Mouth
│   └── Horn
│
├── Mane
│   ├── Mane_Back
│   ├── Mane_Main
│   └── Mane_Front
│
├── Tail
│
├── Legs
│   ├── Front_L
│   ├── Front_R
│   ├── Back_L
│   └── Back_R
│
└── CutieMark
```

Такая структура позволяет анимировать персонажа независимо по частям.

---

# 73. Dynamic mane

Грива не должна быть просто одним статическим изображением.

Она должна поддерживать три режима:

### Static

Обычный SVG path.

### Bone-driven

Грива связана с bones.

### Deformable

Грива имеет mesh/deformation system.

Таким образом одна и та же грива может:

```text
стоять
↓
двигаться при ходьбе
↓
развеваться
↓
сжиматься
↓
деформироваться при лежании
```

---

# 74. Soft-body-like deformation

Для мягких элементов можно использовать simplified spring model.

Каждая control point имеет:

```text
position
velocity
mass
spring
damping
```

Система не обязана быть полноценной физикой.

Цель — визуально естественное движение.

---

# 75. Performance requirements

Целевые показатели:

```text
Editor UI: 60 FPS minimum
Timeline preview: 60 FPS
Drawing latency: < 16 ms
Selection response: < 16 ms
```

Для тяжёлых сцен:

```text
Progressive rendering
Caching
Culling
Multithreading
```

---

# 76. Multithreading

Параллельно могут выполняться:

```text
SVG parsing
Geometry processing
Path tessellation
Animation evaluation
Asset loading
Image decoding
Export encoding
Physics/deformation
```

UI и операции с GUI-state должны выполняться в главном UI thread.

---

# 77. Thread model

```text
Main Thread
│
├── UI
├── Input
├── Window
└── Command Dispatch
       │
       ↓
Job System
├── Geometry Workers
├── Animation Workers
├── Asset Workers
├── Export Workers
└── Physics Workers
```

---

# 78. Plugin API

Программа должна позволять создавать plugins.

Plugin может добавлять:

* tools;
* commands;
* importers;
* exporters;
* panels;
* inspectors;
* animation effects.

Adobe Animate также предоставляет JavaScript API для автоматизации работы с инструментами, а также возможности расширения через C++ libraries. Это хороший ориентир для архитектуры plugin API, хотя интерфейс данного проекта не должен копировать API Adobe.

---

# 79. SVG Inspector

Специальная панель:

```text
<svg>
 ├── <defs>
 ├── <g id="body">
 │    ├── <path>
 │    └── <path>
 └── <g id="head">
      └── <ellipse>
```

Позволяет:

* смотреть структуру;
* менять attributes;
* менять ID;
* менять styles;
* искать элементы;
* временно скрывать элементы.

---

# 80. SVG validation

При загрузке программа должна проверять:

```text
Malformed XML
Missing namespace
Invalid path
Invalid transform
Invalid gradient
Missing reference
Broken href
Circular reference
Unsupported feature
```

Ошибки должны отображаться пользователю понятно:

```text
SVG Import Warning

Element:
    <filter>

Reason:
    Filter type is not supported.

Action:
    Import without filter
```

---

# 81. Security

SVG может содержать потенциально опасные конструкции.

Импортёр должен блокировать:

```text
external scripts
JavaScript
external network resources
unsafe external references
```

если они не разрешены явно пользователем.

---

# 82. Autoscaling и DPI

Редактор должен учитывать:

```text
96 DPI
125%
150%
200%
300%
```

UI не должен зависеть от физического DPI монитора.

---

# 83. Color management

Внутренняя цветовая модель должна поддерживать как минимум:

```text
sRGB
RGBA
```

Архитектура должна позволять в будущем добавить:

```text
Display P3
HDR
16-bit
32-bit float
```

---

# 84. Error handling

Ни одна ошибка отдельного asset не должна приводить к падению всего проекта.

Например:

```text
Broken SVG
     ↓
Import Error
     ↓
Asset marked as invalid
     ↓
Editor continues running
```

---

# 85. Crash recovery

При crash:

```text
Application crash
      ↓
Autosave exists?
      ↓
Yes
      ↓
Recovery dialog
```

---

# 86. Основной принцип редактора

Главный архитектурный принцип:

> **Geometry, appearance, hierarchy и animation должны быть независимыми слоями данных.**

Нельзя делать:

```text
Frame 1 = полностью отдельный SVG
Frame 2 = полностью отдельный SVG
Frame 3 = полностью отдельный SVG
```

для обычной tween-анимации.

Нужно:

```text
Object
│
├── Geometry
├── Style
├── Transform
├── Hierarchy
└── Animation Tracks
```

Это позволяет менять анимацию, не разрушая исходный рисунок.

---

# 87. Итоговая архитектура

```text
                    PROJECT
                       │
                    SCENES
                       │
                  SCENE GRAPH
                       │
        ┌──────────────┼──────────────┐
        │              │              │
     Geometry        Style         Hierarchy
        │              │              │
        └──────────────┼──────────────┘
                       │
                  Animation
                       │
             ┌─────────┴─────────┐
             │                   │
          Timeline             Rig
             │                   │
             └─────────┬─────────┘
                       │
                  Evaluation
                       │
                     Culling
                       │
                  Tessellation
                       │
                    Batching
                       │
                    Renderer
                       │
                     Frame
                       │
                    Export
```

---

# 88. MVP

Первую версию не следует пытаться сразу делать полностью.

### Phase 1

```text
Window
Canvas
Selection
Shapes
Pen
Node editing
Layers
SVG import
SVG export
Save/Open
Undo/Redo
```

### Phase 2

```text
Timeline
Keyframes
Tween
Onion Skin
Camera
```

### Phase 3

```text
Symbols
Rig
Bones
IK
```

### Phase 4

```text
Deformation
Advanced SVG
Masks
Filters
Advanced rendering
```

### Phase 5

```text
Audio
Video export
Plugin API
Advanced debugging
Performance optimization
```

---

# 89. Критическое требование проекта

**SVG не должен быть просто форматом импорта/экспорта.**

Он должен быть одной из фундаментальных частей внутренней графической модели.

То есть пользователь должен иметь возможность:

```text
нарисовать
   ↓
получить SVG-compatible geometry
   ↓
сгруппировать
   ↓
создать персонажа
   ↓
создать rig
   ↓
анимировать
   ↓
экспортировать
```

При этом исходная векторная геометрия должна оставаться доступной для редактирования на любом этапе.

---

# 90. Конечная цель

Программа должна позволять пройти весь путь:

```text
Пустой проект
      ↓
Рисование пони
      ↓
SVG
      ↓
Разделение на части
      ↓
Иерархия
      ↓
Rig
      ↓
Animation
      ↓
Scene
      ↓
Camera
      ↓
Audio
      ↓
Preview
      ↓
Render
      ↓
Готовый мультфильм
```

Главная концепция программы:

> **Не рисовать каждый кадр заново, а создать полноценного векторного персонажа, после чего управлять его геометрией, иерархией и поведением через систему анимации.**

Это позволит использовать один и тот же персонаж в тысячах кадров, сохраняя редактируемость исходной SVG-графики.

---

# 91. Схема project.json и scene.json

Раздел 57 описывает файловую структуру проекта, но не фиксирует формат содержимого. Это необходимо для того, чтобы Scene Graph (раздел 4) и Project (раздел 3) были воспроизводимы разными частями программы (сериализатор, десериализатор, undo-система, плагины) единообразно.

## project.json

```json
{
  "format_version": "1.0",
  "app_version": "1.0.0",
  "project_id": "proj_4a9c...",
  "name": "My Pony Cartoon",
  "created_at": "2026-01-10T12:00:00Z",
  "modified_at": "2026-08-15T09:30:00Z",
  "settings": {
    "width": 1920,
    "height": 1080,
    "fps": 24,
    "background_color": "#00000000",
    "color_space": "sRGB"
  },
  "scenes": [
    { "id": "scene_001", "name": "Intro", "path": "scenes/scene_001.json" }
  ],
  "global_resources": {
    "symbols": ["assets/symbols/eye.json"],
    "audio": ["audio/theme.mp3"]
  },
  "asset_index": "assets/assets.json"
}
```

## scene.json

```json
{
  "scene_id": "scene_001",
  "name": "Intro",
  "duration_frames": 720,
  "fps": 24,
  "layers": [
    {
      "id": "layer_001",
      "type": "vector",
      "name": "Pony",
      "visible": true,
      "locked": false,
      "opacity": 1.0,
      "blend_mode": "normal",
      "objects": ["obj_8f31..."],
      "timeline": "timelines/layer_001.json"
    }
  ],
  "camera": "camera_001"
}
```

Каждый объект сцены сериализуется отдельной записью (или отдельным файлом при больших сценах) и ссылается на другие объекты **только по `object_id`**, никогда по имени или индексу в массиве — это согласуется с требованием раздела 3 об уникальных ID.

```json
{
  "object_id": "obj_8f31...",
  "type": "path",
  "parent": "obj_1a02...",
  "geometry": "geometry_ref_or_inline",
  "style": { "fill": "#608BFF", "stroke": null },
  "transform": { "x": 0, "y": 0, "rotation": 0, "scale_x": 1, "scale_y": 1, "skew_x": 0, "skew_y": 0, "pivot_x": 0.5, "pivot_y": 0.5 },
  "visible": true,
  "opacity": 1.0,
  "blend_mode": "normal",
  "z_order": 3,
  "animation_tracks": "tracks_ref",
  "metadata": {}
}
```

Хранение geometry «inline» либо «by reference» (в отдельном .path-файле/asset) должно определяться размером объекта — эвристика описана в разделе 92.

---

# 92. Версионирование и миграция формата

Проектный формат будет меняться со временем, поэтому необходима явная схема совместимости.

```text
project.json
├── format_version   — версия схемы данных (например "1.3")
└── app_version       — версия приложения, сохранившего файл
```

Правила:

* при открытии проекта с `format_version` ниже текущей — запускается цепочка миграторов;
* каждый миграт (`migrate_1_0_to_1_1`, `migrate_1_1_to_1_2`, …) применяется последовательно;
* при открытии проекта с `format_version` **выше** текущей версии приложения — показывается предупреждение:

```text
Project was created in a newer version of the app.
Some features may not load correctly.
```

* оригинальный файл при миграции не перезаписывается автоматически — сохраняется как `project.json.bak` перед первым сохранением в новом формате.

```text
Open old project
      ↓
Detect format_version
      ↓
Run migration chain
      ↓
Backup original
      ↓
Load into memory
```

---

# 93. Skinning (привязка геометрии к костям)

Раздел 40 вводит кости, раздел 42 упоминает "Bone Deformation" как один из вариантов деформации, но не описывает сам механизм привязки геометрии к скелету. Это необходимо детализировать, так как именно skinning определяет, как деформируется Mesh при движении костей.

Поддерживаемые режимы привязки:

```text
Rigid Binding      — объект целиком следует одной кости (родитель = bone)
Weighted Binding    — каждая вершина/control point path имеет веса на 1..N костей
Region Binding      — объект разбит на непересекающиеся регионы, каждый привязан к своей кости
```

## Rigid Binding

Простейший случай — используется для жёстких частей (копыта, глаза, cutie mark).

```text
FrontLeg (path)
   parent = Bone_FrontLeg
```

## Weighted Binding (Mesh Deformation)

Каждая вершина хранит список пар `(bone_id, weight)`, сумма весов = 1.0:

```text
Vertex_12:
  Bone_Shoulder: 0.7
  Bone_UpperArm: 0.3
```

Итоговая позиция вершины вычисляется как взвешенная сумма трансформаций костей (linear blend skinning). Архитектура должна допускать замену этого алгоритма (например, dual quaternion skinning) без изменения формата хранения весов.

## Region Binding

Компромисс между Rigid и Weighted — упрощённый вариант для 2D-мультипликации, где объект делится на именованные регионы (аналог "cutout" риггинга):

```text
Mane
├── Region_Top    → Bone_Mane_01
├── Region_Mid    → Bone_Mane_02
└── Region_Tip    → Bone_Mane_03
```

Каждый регион при этом может дополнительно иметь Bezier Deformation (раздел 74) поверх base skinning для сглаживания стыков между регионами.

Инструмент **Weight Paint** должен позволять:

* рисовать веса кистью;
* нормализовать веса;
* показывать heatmap весов на объекте;
* автоматически рассчитывать начальные веса (auto-weights) по расстоянию до костей.

---

# 94. Constraints

Раздел 40 указывает, что кость может иметь поле `constraints`, но не раскрывает их типы. IK (раздел 41) — лишь один из видов constraint.

Поддерживаемые типы:

```text
IK Constraint          — см. раздел 41
Look-At Constraint     — кость поворачивается в сторону target-объекта
Path Constraint        — объект/кость следует вдоль заданного path
Parent Constraint      — временное переопределение родителя без изменения иерархии
Position Constraint    — ограничение позиции по осям/диапазону
Rotation Constraint    — ограничение угла поворота (min/max)
Scale Constraint       — ограничение масштаба
Distance Constraint     — поддержание фиксированного расстояния до target
```

Каждый constraint должен иметь:

```text
type
target
weight (0..1, для смешивания с базовой анимацией)
enabled
space (local / world / parent)
```

Constraints вычисляются **после** базовой анимации, но **до** финального world transform:

```text
Base Transform (animation tracks)
        ↓
Constraints (в порядке стека)
        ↓
Final World Transform
```

Это позволяет, например, анимировать ногу покадрово, а затем наложить IK-constraint только на часть таймлайна.

---

# 95. Symbol instance overrides

Раздел 28 описывает Symbol как переиспользуемый объект (`Eye` → `Eye_L`, `Eye_R`), но не покрывает случай, когда конкретному инстансу нужно локальное отличие от исходника — это часто необходимо в риггинге персонажей.

Каждый Symbol Instance может содержать необязательный блок `overrides`:

```json
{
  "type": "symbol_instance",
  "symbol_ref": "symbol_eye",
  "instance_id": "obj_eye_l",
  "transform": { "x": 120, "y": 40, "scale_x": -1 },
  "overrides": {
    "fill": "#3AA6FF",
    "visible_children": ["pupil", "highlight"]
  }
}
```

Правила:

* `overrides` не изменяют исходный Symbol Definition;
* при редактировании Symbol Definition изменения применяются ко всем инстансам, **кроме** переопределённых свойств;
* Symbol Instance может дополнительно "разорвать" связь с определением через `Modify → Break Apart Symbol` — после этого объект становится независимой копией геометрии (обычная группа), больше не связанной с Symbol.

Это соответствует зеркалированию `Eye_L`/`Eye_R` через `scale_x: -1` без создания второго Symbol Definition.

> **Статус реализации:** частично. `VectorShape::Instance.fill_override`
> реализует ровно правило "overrides не изменяют исходный Symbol
> Definition, но переопределяют заливку для этого инстанса" — проверено
> тестом (`resolve_symbol_instance_fill_override_recolors_without_touching_
> definition`). `transform` (позиция/поворот/масштаб, включая
> зеркалирование через отрицательный scale) — полноценно реализован как
> собственное поле инстанса, а не через overrides-блок. `Modify → Break
> Apart Symbol` — реализован (`VectorDoc::break_apart_symbol_instance`).
> НЕ реализовано в этом проходе: `overrides.visible_children` (точечное
> скрытие отдельных вложенных фигур символа для конкретного инстанса) и
> per-instance override обводки (только заливка) — оба покрывают более
> редкие случаи, чем "перекрасить весь инстанс", отдельная задача при
> первом реальном запросе на неё.

---

# 96. Text object (архитектурный минимум)

Раздел 49 указывает, что текстовый инструмент не приоритет, но раздел 5 включает `text` как поддерживаемый SVG-элемент. Чтобы архитектура действительно "оставляла возможность добавить text object позднее" (как требует раздел 49), необходимо заранее зафиксировать минимальную модель данных — иначе добавление текста задним числом потребует переделки Scene Graph.

Text object должен хранить:

```text
content        — строка (с поддержкой multi-line)
font_family
font_size
font_weight
font_style     — normal / italic
alignment      — left / center / right / justify
line_height
letter_spacing
fill
stroke
```

Ключевое архитектурное решение: text остаётся **редактируемым текстом** (не автоконвертируется в path) до явной команды `Convert to Path` (аналогично разделу 48, "Convert to Path" для фигур). Это гарантирует, что:

* шрифт можно менять после ввода;
* при экспорте без embedded fonts программа предупреждает пользователя и предлагает конвертацию в path;
* Scene Graph узел text ведёт себя как обычный animatable object (позиция, поворот, opacity, animation tracks — без изменений остальной модели).

Embedding/subsetting шрифтов при экспорте — отдельная задача Export Pipeline (раздел 65) и не блокирует MVP.

---

# 97. Localization (i18n) интерфейса

Документ не описывает локализацию UI — при этом Help/Documentation (раздел 54) и любые диалоги (раздел 80, 85) должны иметь возможность перевода без изменения кода.

Минимальные требования:

```text
Все строки UI вынесены в resource-файлы (например .json / .po)
Формат: locale_code → { key: translated_string }
Fallback: если перевод отсутствует → английский язык по умолчанию
```

```text
locales/
├── en.json
├── ru.json
└── ...
```

Числа, даты и единицы измерения (frame rate, px/units) должны форматироваться с учётом локали, но **внутренние данные проекта (project.json) всегда хранятся в инвариантном формате** (точка как десятичный разделитель, ISO-даты) — локализуется только отображение.

---

# 98. Настраиваемые горячие клавиши

Раздел 67 задаёт фиксированный список хоткеев, раздел 45 упоминает Preferences, но не описывает, что именно в них настраивается. Пользовательская настройка хоткеев обязательна для профессионального инструмента.

Preferences должны включать панель **Keyboard Shortcuts**, позволяющую:

* переназначить любую команду на другую комбинацию клавиш;
* видеть конфликты назначений;
* сохранять/загружать наборы хоткеев (presets), включая пресет "Adobe Animate-like" для облегчения перехода пользователей (см. раздел 1 — сама программа не копия Animate, но совместимый пресет хоткеев не противоречит этому);
* сбросить к значениям по умолчанию.

Формат хранения — отдельный `keymap.json` в пользовательских настройках (не в project.json, так как это настройка приложения, а не проекта):

```json
{
  "preset": "default",
  "overrides": {
    "tool.selection": "V",
    "command.undo": "Ctrl+Z"
  }
}
```

---

# 99. Color palette / Swatches

Раздел 53 упоминает панель Color в Window menu, но не описывает её как подсистему. Для работы над персонажем с устойчивой цветовой схемой (например, фиксированные цвета гривы, тела, cutie mark у каждой версии персонажа) нужны сохраняемые палитры.

Swatches panel должна поддерживать:

```text
Project Palette     — цвета, привязанные к текущему проекту
Global Palette      — переиспользуемые пользовательские палитры (между проектами)
Recent Colors
```

Каждый swatch:

```text
id
color (RGBA/HEX/HSL)
name (опционально, например "Mane Purple")
```

Важное свойство: цвет объекта может либо хранить **литеральное значение**, либо **ссылку на swatch**. Во втором случае изменение swatch обновляет все объекты, ссылающиеся на него — аналогично Symbol (раздел 28), но для стилей, а не геометрии.

```json
{ "fill": { "swatch_ref": "swatch_mane_purple" } }
```

vs.

```json
{ "fill": { "color": "#7A4FBE" } }
```

---

# 100. Зависимости между assets и сценами

Раздел 56 указывает, что каждый asset хранит поле `dependencies`, но не поясняет, как решается конфликт использования одного Symbol в нескольких сценах.

Правила:

* Symbol Definitions и другие Global Resources (раздел 3) хранятся один раз в `Global Resources`, независимо от количества сцен, использующих их;
* каждая сцена хранит только **ссылки** (`object_id` инстанса → `symbol_ref`), не копии;
* при удалении Symbol Definition, на который есть ссылки, редактор должен:

```text
Symbol "Eye" is used in 2 scenes (14 instances).
[ ] Delete anyway (instances become broken references)
[ ] Convert instances to independent objects first
[ ] Cancel
```

* Asset Manager должен уметь строить граф зависимостей для команды "Find Unused Assets" (полезно вместе с Commands из раздела 50, например "Remove Invisible Objects" по аналогии) и для корректного порядка загрузки при открытии проекта.

```text
project.json
   ↓
asset_index
   ↓
Dependency Graph
   ↓
Topological Load Order
```

---

# 101. Coordinate spaces

Раздел 4 упоминает `local transform` и `world transform`, раздел 13 задаёт порядок Translate → Rotate → Scale → Skew, но не описывает явно, как вычисляется world transform в иерархии и как в неё встраивается камера.

Формально:

```text
World Transform(object) = World Transform(parent) × Local Transform(object)
```

Порядок систем координат от объекта до экрана:

```text
Object Local Space
      ↓ (local transform)
Parent Space
      ↓ (рекурсивно до корня сцены)
Scene Space
      ↓ (camera transform, раздел 43)
Camera Space
      ↓ (viewBox / viewport mapping, раздел 7)
Screen Space
```

Pivot (раздел 14) применяется **внутри** local transform, до умножения на transform родителя:

```text
Local Transform = Translate(pivot) × Rotate × Scale × Skew × Translate(-pivot)
```

Это должно быть зафиксировано явно, так как Rigging (skinning, раздел 93) и IK (раздел 41) требуют многократного пересчёта world transform по всей цепочке костей за кадр, и любая неоднозначность в порядке умножения матриц ведёт к визуально неверной деформации.

---

# 102. Совместная работа / контроль версий (вне скоупа v1.0)

Так как раздел 57 сравнивает проектный формат с раздельным FLA/XFL-подходом Adobe "удобным для совместной работы", стоит явно зафиксировать текущий скоуп, чтобы не создавать ложных ожиданий.

```text
Не входит в v1.0:
    real-time multi-user editing
    built-in cloud sync
    built-in merge tool для конфликтов
```

Что архитектура **обязана** обеспечить уже сейчас, чтобы не блокировать это в будущем:

* project-как-набор-файлов (уже в разделе 57) — совместим с обычным git;
* JSON, а не бинарный формат — совместим с текстовым diff;
* стабильные `object_id` (раздел 3) — не переиспользуются и не зависят от порядка сохранения, что критично для минимизации git-конфликтов между версиями одного файла.

---

# 103. Обновлённый MVP (Phase 0)

Раздел 88 начинает MVP сразу с Phase 1 (Window/Canvas/Selection...). Стоит явно выделить Phase 0 — инфраструктурный фундамент, без которого невозможна ни одна из последующих фаз, включая формат данных из раздела 91.

### Phase 0 — Data Foundation

```text
object_id generation
project.json / scene.json schema (раздел 91)
format_version + migration stub (раздел 92)
Scene Graph core (создание/удаление/reparent)
Local/World transform pipeline (раздел 101)
Command Pattern skeleton (для Undo/Redo, раздел 45)
```

Только после Phase 0 имеет смысл начинать Phase 1, так как Canvas, Selection и SVG import/export (Phase 1) все опираются на уже стабильную модель данных, а не наоборот.
