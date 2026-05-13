# frozen_string_literal: true

# Internal canonical dump used by the compatibility specs. Walks an
# `RBS::Environment` and emits a deterministic, line-oriented summary of
# its six `*_decls` tables so two environments can be compared byte for
# byte. The format is not a public contract — only this file produces
# it, only the compat specs consume it. Tweak both together when
# adjusting the format.
#
# Format sketch (UTF-8, `\n`-terminated lines, no trailing whitespace):
#
#   == class_decls ==        <- one section header per `*_decls` hash
#   class ::Foo              <- entries sorted by `name.to_s` raw bytes
#   module ::Bar             <- `module` for ModuleEntry, `class` otherwise
#   == interface_decls ==
#   interface ::_Each
#   == type_alias_decls ==
#   == constant_decls ==
#   constant ::PI
#   == class_alias_decls ==
#   class_alias ::A = ::B    <- `module_alias` for ModuleAliasEntry
#   == global_decls ==
#   global $LOAD_PATH        <- key is the symbol literal, including `$`

require "rbs"

# Walk a real `RBS::Environment` and emit the canonical dump string.
def canonical_dump(env)
  out = +""
  dump_class_decls(env, out)
  dump_interface_decls(env, out)
  dump_type_alias_decls(env, out)
  dump_constant_decls(env, out)
  dump_class_alias_decls(env, out)
  dump_global_decls(env, out)
  out
end

def dump_class_decls(env, out)
  out << "== class_decls ==\n"
  sorted_by_name(env.class_decls).each do |name, entry|
    kind = entry.is_a?(RBS::Environment::ModuleEntry) ? "module" : "class"
    out << "#{kind} #{name}\n"
  end
end

def dump_interface_decls(env, out)
  out << "== interface_decls ==\n"
  sorted_by_name(env.interface_decls).each do |name, _|
    out << "interface #{name}\n"
  end
end

def dump_type_alias_decls(env, out)
  out << "== type_alias_decls ==\n"
  sorted_by_name(env.type_alias_decls).each do |name, _|
    out << "type_alias #{name}\n"
  end
end

def dump_constant_decls(env, out)
  out << "== constant_decls ==\n"
  sorted_by_name(env.constant_decls).each do |name, _|
    out << "constant #{name}\n"
  end
end

def dump_class_alias_decls(env, out)
  out << "== class_alias_decls ==\n"
  sorted_by_name(env.class_alias_decls).each do |name, entry|
    kind = entry.is_a?(RBS::Environment::ModuleAliasEntry) ? "module_alias" : "class_alias"
    out << "#{kind} #{name} = #{entry.decl.old_name}\n"
  end
end

def dump_global_decls(env, out)
  out << "== global_decls ==\n"
  # Global keys are Symbols (e.g. :$LOAD_PATH); sort by their string form.
  env.global_decls.keys.sort_by(&:to_s).each do |sym|
    out << "global #{sym}\n"
  end
end

# Sort an entry hash by `name.to_s` (raw byte order).
def sorted_by_name(decls)
  decls.sort_by { |name, _| name.to_s }
end
