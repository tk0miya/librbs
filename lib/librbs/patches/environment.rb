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

      # Each of the six `*_decls` accessors triggers a one-shot
      # materialization on first access, then defers to the upstream
      # ivar reader. `super()` ends up reading the
      # `RBS::Environment::ClassEntry` etc. hash that
      # `Librbs::Native.materialize_all` just wrote onto the instance.
      # `sources` and `declarations` are part of the same source-derived
      # API surface and get the same treatment so the @sources ivar
      # populated by materialization is observed.
      %i[class_decls interface_decls type_alias_decls
         constant_decls class_alias_decls global_decls
         sources declarations].each do |m|
        define_method(m) do
          ensure_materialized
          super()
        end
      end

      # Source-iterator accessors fan out from `@sources`; trigger
      # materialization the same way before delegating upstream.
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

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
RBS::Environment.prepend(Librbs::Patches::Environment)
