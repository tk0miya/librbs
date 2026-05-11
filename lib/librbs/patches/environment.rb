# frozen_string_literal: true

module Librbs
  module Patches
    module Environment
      # `RBS::Environment#resolve_type_names(only: nil)` — replaced by an
      # M3d native call. The Set-of-TypeName form upstream takes is
      # converted to an Array here so the magnus side can iterate it
      # using only `RArray` C-API calls (no Ruby method dispatch). When
      # `only` is `nil` every declaration is resolved, matching the
      # upstream default.
      #
      # Pure-Ruby `RBS::Environment.new` instances (no `@__librbs_handle`)
      # fall through to upstream. The native path is sound only when the
      # handle's `Arc<Environment>` has strong count 1, which is
      # established by `from_loader`; an env populated via `add_source`
      # on the Ruby side has no Rust state to resolve against and must
      # use upstream's `resolve_type_names`.
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
