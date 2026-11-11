; Kotlin highlights for tree-sitter-kotlin-ng, which ships none. Written for Shed.
[(line_comment) (block_comment)] @comment
[(string_literal) (multiline_string_literal) (character_literal)] @string
[(number_literal) (float_literal)] @number
(annotation) @attribute
(label) @label
(user_type (identifier) @type)
(class_declaration (identifier) @type)
(function_declaration (identifier) @function)
[
  "abstract" "actual" "as" "by" "catch" "class" "companion" "const" "constructor" "crossinline"
  "data" "do" "else" "enum" "expect" "external" "final" "finally" "for" "fun" "get" "if" "import"
  "in" "infix" "init" "inline" "inner" "interface" "internal" "is" "lateinit" "noinline" "object"
  "open" "operator" "out" "override" "package" "private" "protected" "public" "return" "sealed"
  "set" "super" "suspend" "tailrec" "this" "throw" "try" "typealias" "val" "value" "var" "vararg"
  "when" "where" "while"
] @keyword
