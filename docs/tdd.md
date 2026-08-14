# Pony Animation Studio — Technical Design Document

**Версия:** 1.0
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
