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
    end
  end
end

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
RBS::Environment.prepend(Librbs::Patches::Environment)
