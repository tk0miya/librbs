# frozen_string_literal: true

module Librbs
  module Patches
    module Environment
      module ClassMethods
        def from_loader(loader)
          Librbs::Native.build_environment(loader)
        end
      end
    end
  end
end

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
