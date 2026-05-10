# frozen_string_literal: true

module Librbs
  module Patches
    module Environment
      module ClassMethods
        def from_loader(loader)
          Librbs::Native.build_environment(loader)
        end
      end

      # `RBS::Environment#resolve_type_names(only: nil)` — replaced by an
      # M3d native call. The Set-of-TypeName form upstream takes is
      # converted to an Array here so the magnus side can iterate it
      # using only `RArray` C-API calls (no Ruby method dispatch). When
      # `only` is `nil` every declaration is resolved, matching the
      # upstream default.
      def resolve_type_names(only: nil)
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
      #
      # GC is disabled for the duration of `materialize_all` because the
      # native side allocates ~2.3 M short-lived Ruby objects per pass
      # (Location, TypeName, Namespace, Types::*) and the resulting GC
      # pressure dominates wall-clock time — disabling GC here roughly
      # halves materialise time on the `large` benchmark fixture.
      # `GC.disable` returns the previous state, so a caller that had
      # already disabled GC keeps that state on exit.
      def ensure_materialized
        return if @__librbs_materialized
        return unless instance_variable_defined?(:@__librbs_handle)
        was_disabled = GC.disable
        begin
          Librbs::Native.materialize_all(self)
        ensure
          GC.enable unless was_disabled
        end
      end
    end
  end
end

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
RBS::Environment.prepend(Librbs::Patches::Environment)
