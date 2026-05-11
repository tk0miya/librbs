# frozen_string_literal: true

module Librbs
  module Patches
    module Environment
      # `RBS::Environment#resolve_type_names(only: nil)` — replaced by a
      # native call. The Set-of-TypeName form upstream takes is converted
      # to an Array here so the magnus side can iterate it using only
      # `RArray` C-API calls (no Ruby method dispatch). When `only` is
      # `nil` every declaration is resolved, matching the upstream
      # default.
      #
      # Pure-Ruby `RBS::Environment.new` instances (no `@__librbs_handle`)
      # fall through to upstream. The native path clones the wrapped
      # `Arc<Environment>` internally (see `ext/librbs/src/lib.rs`'s
      # `resolve_type_names`), so a shared handle — e.g. one inherited
      # by `dup` — is sound. An env populated via `add_source` on the
      # Ruby side still has no Rust counterpart for the user-added
      # source, but the resolver is happy to operate on the pre-add
      # state held by the handle.
      def resolve_type_names(only: nil)
        unless instance_variable_defined?(:@__librbs_handle)
          return super
        end

        only_array = only.nil? ? nil : only.to_a
        Librbs::Native.resolve_type_names(self, only_array)
      end

      # Each source-derived accessor triggers a one-shot materialization
      # on first access, then defers to the upstream ivar reader.
      # `super()` ends up reading the `@sources` Array and the six
      # `*_decls` Hashes that upstream `add_source` wrote during
      # materialisation. `declarations` is `attr_reader`-less upstream
      # (defined as `sources.flat_map(&:declarations)`); we trigger
      # materialisation and let the upstream method recompute.
      %i[class_decls interface_decls type_alias_decls
         constant_decls class_alias_decls global_decls
         sources declarations].each do |m|
        define_method(m) do
          ensure_materialized
          super()
        end
      end

      # Block-taking variants need an explicit `&block` shuttle so
      # `super(&block)` passes through the caller's block unchanged.
      %i[each_rbs_source each_ruby_source].each do |m|
        define_method(m) do |&block|
          ensure_materialized
          super(&block)
        end
      end

      # Upstream `inspect` reads `@class_decls.size` etc. via
      # `instance_variable_get`, bypassing the patched accessors.
      # Without this trigger, `pp env` before any other read prints
      # `(0 items)` for every category.
      def inspect
        ensure_materialized
        super
      end

      # Upstream `initialize_copy` only dups `@sources` and the six
      # decl Hashes — it never touches `@__librbs_handle` or
      # `@__librbs_materialized`. The shared `Arc<Environment>` rides
      # along via the shallow `Object#dup` copy, which is fine now
      # that `librbs_core::Environment` is `Clone`: the native paths
      # (resolve_type_names, materialize_all) clone the Arc when they
      # need exclusive ownership, so sharing the handle is sound.
      #
      # Two ordering hazards force this patch:
      #   1. `Object#dup` shallow-copies *every* ivar at dup time —
      #      including `@__librbs_materialized`. If `other` was
      #      unmaterialized then, the dup walks away with the flag
      #      unset and would later re-run `materialize_all` against
      #      its already-populated Ruby ivars, double-counting
      #      `@sources`. We materialize `other` first, then stamp
      #      `@__librbs_materialized = true` on self after `super` to
      #      lock the dup's view to the just-dup'd Ruby state.
      #   2. Matching upstream's pure-Ruby contract requires
      #      `dup.sources[i].equal?(env.sources[i])`. Materializing
      #      `other` before `super` is what produces the `Source::RBS`
      #      Ruby objects that both arrays now share.
      def initialize_copy(other)
        other.send(:ensure_materialized)
        super
        @__librbs_materialized = true if instance_variable_defined?(:@__librbs_handle)
      end

      # `add_source` and `unload` mutate the Ruby ivars directly.
      # Materialise first so the user mutation lands on top of the
      # Rust-side state instead of being overwritten by a later
      # materialisation.
      def add_source(source)
        ensure_materialized
        super
      end

      def unload(paths)
        ensure_materialized
        super
      end

      private

      # Pure-Ruby `RBS::Environment.new` instances have no Rust handle;
      # the `instance_variable_defined?` guard preserves their
      # accessor's no-op fast path (the upstream initializer already
      # set `@class_decls = {}` etc.).
      def ensure_materialized
        return if @__librbs_materialized
        return unless instance_variable_defined?(:@__librbs_handle)
        Librbs::Native.materialize_all(self)
      end
    end
  end
end

RBS::Environment.prepend(Librbs::Patches::Environment)
