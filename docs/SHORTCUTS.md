# Keyboard shortcuts

The bindings this class of editor has used for decades, because that is what
hands already know. Where a conventional shortcut has no counterpart in C-Shop
it is left unbound rather than pointed at something that merely resembles it —
the unbound ones are listed at the end.

Every chord below lives in one table in `cshop-ui/src/shortcuts.rs`. The menus
print their accelerators from that same table, and a test fails if a chord is
named but nothing runs it.

## Tools

| Key | Tool |
|---|---|
| `V` | Move |
| `M` | Rectangular / Elliptical Marquee |
| `L` | Lasso / Polygonal Lasso |
| `W` | Magic Wand |
| `C` | Crop |
| `I` | Eyedropper |
| `B` | Brush / Pencil |
| `S` | Clone Stamp |
| `E` | Eraser |
| `G` | Paint Bucket / Gradient |
| `T` | Type |
| `U` | Shape |
| `H` | Hand |
| `Z` | Zoom |

Pressing a letter again cycles through the tools sharing that slot.

## File and edit

| Chord | Command |
|---|---|
| `Ctrl+N` | New |
| `Ctrl+O` | Open |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save As |
| `Ctrl+W` | Close document |
| `Ctrl+Q` | Quit |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / Redo |
| `Ctrl+Alt+Z` | Step backward |
| `Ctrl+C` / `Ctrl+X` | Copy / Cut |
| `Ctrl+Shift+C` | Copy Merged — every visible layer, not just the active one |
| `Ctrl+V` | Paste onto a new layer, centred |
| `Ctrl+Shift+V` | Paste in Place — back where it was copied from |
| `Ctrl+T` | Free Transform |
| `Ctrl+F` | Repeat last filter |

## Fill

| Chord | Command |
|---|---|
| `Shift+Backspace` | Fill dialog |
| `Alt+Backspace` | Fill with foreground |
| `Ctrl+Backspace` | Fill with background |
| `Shift+Alt+Backspace` | Fill foreground, preserving transparency |
| `Ctrl+Shift+Backspace` | Fill background, preserving transparency |
| `Delete` | Clear |

## Image and adjustments

| Chord | Command |
|---|---|
| `Ctrl+Alt+I` / `Ctrl+Alt+C` | Image Size / Canvas Size |
| `Ctrl+L` | Levels |
| `Ctrl+M` | Curves |
| `Ctrl+U` | Hue/Saturation |
| `Ctrl+B` | Color Balance |
| `Ctrl+Shift+U` | Desaturate |
| `Ctrl+I` | Invert |

## Layers

| Chord | Command |
|---|---|
| `Ctrl+Shift+N` | New layer |
| `Ctrl+J` | Layer via Copy — with a selection, only the selected pixels |
| `Ctrl+E` | Merge down |
| `Ctrl+Alt+G` | Toggle clipping mask |
| `Ctrl+]` / `Ctrl+[` | Move layer up / down the stack |
| `Ctrl+Shift+]` / `Ctrl+Shift+[` | Move to top / bottom |
| `Alt+]` / `Alt+[` | Select the layer above / below |

## Selection

| Chord | Command |
|---|---|
| `Ctrl+A` | Select all |
| `Ctrl+D` / `Ctrl+Shift+D` | Deselect / Reselect |
| `Ctrl+Shift+I` | Inverse |
| `Shift+F6` | Feather |
| `Q` | Quick Mask |

While a selection tool is active, `Shift` adds, `Alt` subtracts, and both
together intersect.

## View and colour

| Chord | Command |
|---|---|
| `Ctrl++` / `Ctrl+-` | Zoom in / out |
| `Ctrl+0` / `Ctrl+1` | Fit on screen / actual pixels |
| `Tab` | Show or hide the panels |
| `X` | Swap foreground and background |
| `D` | Reset to black and white |
| Hold `Space` | Temporary Hand tool |

## Painting

| Chord | Command |
|---|---|
| `[` / `]` | Smaller / larger brush |
| `Shift+[` / `Shift+]` | Softer / harder brush |
| `1`–`9`, `0` | Opacity 10%–100% |
| `Alt`+wheel | Resize the brush |
| `Alt`+click | Set the clone stamp source |

## Type

While a type layer is being edited the keyboard belongs to it, so the tool
letters type letters.

| Chord | Command |
|---|---|
| `Enter` | New line |
| `Ctrl+Enter` | Commit |
| `Esc` | Abandon the edit |
| Arrows, `Home`, `End` | Move the caret |

## Shapes

Drag to draw. `Shift` constrains to a square or circle, `Alt` draws out from
the centre.

## Deliberately unbound

`Ctrl+Shift+E` (Merge Visible), `Ctrl+G` / `Ctrl+Shift+G`
(group and ungroup — the New Group command here creates an *empty* group, which
is a different action), `Ctrl+H` (Hide Extras), rulers and guides.
