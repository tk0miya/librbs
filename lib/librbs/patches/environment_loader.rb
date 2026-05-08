# frozen_string_literal: true

module Librbs
  module Patches
    module EnvironmentLoader
      module ClassMethods
        # No overrides yet; the loader is consumed entirely by
        # `Librbs::Native.build_environment` via ivar reads. Reserved as an
        # explicit anchor so M3d / M3e can hook in without restructuring
        # the patch tree.
      end
    end
  end
end
