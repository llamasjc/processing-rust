;; Rust highlighting for Rust Mode.
;;
;; The first part is highlights.scm from tree-sitter-rust (MIT license,
;; https://github.com/tree-sitter/tree-sitter-rust), unchanged except for one
;; stray quote in the constant pattern. The Processing part at the end is
;; generated from keywords.txt.
;;
;; Keep this file ASCII: java-tree-sitter 1.9.1 cuts the query short by one
;; byte for every non-ASCII character in it.

; Identifier conventions

; Assume all-caps names are constants
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z\\d_]+$"))

; Assume that uppercase names in paths are types
((scoped_identifier
  path: (identifier) @type)
 (#match? @type "^[A-Z]"))
((scoped_identifier
  path: (scoped_identifier
    name: (identifier) @type))
 (#match? @type "^[A-Z]"))
((scoped_type_identifier
  path: (identifier) @type)
 (#match? @type "^[A-Z]"))
((scoped_type_identifier
  path: (scoped_identifier
    name: (identifier) @type))
 (#match? @type "^[A-Z]"))

; Assume other uppercase names are enum constructors
((identifier) @constructor
 (#match? @constructor "^[A-Z]"))

; Assume all qualified names in struct patterns are enum constructors. (They're
; either that, or struct names; highlighting both as constructors seems to be
; the less glaring choice of error, visually.)
(struct_pattern
  type: (scoped_type_identifier
    name: (type_identifier) @constructor))

; Function calls

(call_expression
  function: (identifier) @function)
(call_expression
  function: (field_expression
    field: (field_identifier) @function.method))
(call_expression
  function: (scoped_identifier
    "::"
    name: (identifier) @function))

(generic_function
  function: (identifier) @function)
(generic_function
  function: (scoped_identifier
    name: (identifier) @function))
(generic_function
  function: (field_expression
    field: (field_identifier) @function.method))

(macro_invocation
  macro: (identifier) @function.macro
  "!" @function.macro)

; Function definitions

(function_item (identifier) @function)
(function_signature_item (identifier) @function)

; Other identifiers

(type_identifier) @type
(primitive_type) @type.builtin
(field_identifier) @property

(line_comment) @comment
(block_comment) @comment

"(" @punctuation.bracket
")" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket

(type_arguments
  "<" @punctuation.bracket
  ">" @punctuation.bracket)
(type_parameters
  "<" @punctuation.bracket
  ">" @punctuation.bracket)

"::" @punctuation.delimiter
":" @punctuation.delimiter
"." @punctuation.delimiter
"," @punctuation.delimiter
";" @punctuation.delimiter

(parameter (identifier) @variable.parameter)

(lifetime (identifier) @label)

"as" @keyword
"async" @keyword
"await" @keyword
"break" @keyword
"const" @keyword
"continue" @keyword
"default" @keyword
"dyn" @keyword
"else" @keyword
"enum" @keyword
"extern" @keyword
"fn" @keyword
"for" @keyword
"if" @keyword
"impl" @keyword
"in" @keyword
"let" @keyword
"loop" @keyword
"macro_rules!" @keyword
"match" @keyword
"mod" @keyword
"move" @keyword
"pub" @keyword
"ref" @keyword
"return" @keyword
"static" @keyword
"struct" @keyword
"trait" @keyword
"type" @keyword
"union" @keyword
"unsafe" @keyword
"use" @keyword
"where" @keyword
"while" @keyword
(crate) @keyword
(mutable_specifier) @keyword
(use_list (self) @keyword)
(scoped_use_list (self) @keyword)
(scoped_identifier (self) @keyword)
(super) @keyword

(self) @variable.builtin

(char_literal) @string
(string_literal) @string
(raw_string_literal) @string

(boolean_literal) @constant.builtin
(integer_literal) @constant.builtin
(float_literal) @constant.builtin

(escape_sequence) @escape

(attribute_item) @attribute
(inner_attribute_item) @attribute

"*" @operator
"&" @operator
"'" @operator

;; -- Processing API ---------------------------------------------------------
((identifier) @function.builtin
  (#match? @function.builtin
    "^(load_pixels|update_pixels|loadPixels|updatePixels|abs|apply_matrix|arc|arc_mode|background|background_color|background_rgb|background_rgba|blendMode|blend_mode|ceil|circle|color|color_rgb|color_rgba|constrain|cursor|day|degrees|dist|ellipse|ellipseMode|ellipse_mode|exit|exp|fill|fill_color|fill_rgb|fill_rgba|floor|frameRate|frame_rate|fullScreen|full_screen|get_pixel|hour|imageMode|image_mode|lerp|lerpColor|lerp_color|line|log|loop|mag|map|max|min|minute|month|noCursor|noFill|noLoop|noSmooth|noStroke|no_cursor|no_fill|no_loop|no_smooth|no_stroke|noise|noise2|point|popMatrix|popStyle|pop_matrix|pop_style|pow|print|println|pushMatrix|pushStyle|push_matrix|push_style|quad|radians|random|randomSeed|random_range|random_seed|rect|rectMode|rectRounded|rect_mode|rect_rounded|redraw|rotate|round|scale|second|set_pixel|shearX|shearY|shear_x|shear_y|size|smooth|sqrt|square|stroke|strokeCap|strokeJoin|strokeWeight|stroke_cap|stroke_color|stroke_join|stroke_rgb|stroke_rgba|stroke_weight|text|textSize|textWidth|text_size|text_width|tint_ga|tint_rgb|translate|triangle|windowResizable|windowTitle|window_resizable|window_title|year)$"))

((identifier) @variable.builtin
  (#match? @variable.builtin
    "^(ADD|BACKSPACE|BEVEL|BLEND|CENTER|CHORD|CONTROL|CORNER|CORNERS|DARKEST|DELETE|DIFFERENCE|DOWN|END|ENTER|ESCAPE|EXCLUSION|F1|F10|F11|F12|F2|F3|F4|F5|F6|F7|F8|F9|HALF_PI|HOME|LEFT|LIGHTEST|MITER|MULTIPLY|OPEN|PAGE_DOWN|PAGE_UP|PI|PIE|PROJECT|QUARTER_PI|RADIUS|REPLACE|RIGHT|ROUND|SCREEN|SHIFT|SQUARE|SUBTRACT|TAB|TWO_PI|UP)$"))
