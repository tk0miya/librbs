# frozen_string_literal: true

module Librbs
  module Patches
    # Defer `RBS::Location` construction itself — not just its children.
    #
    # The native materialiser stores a 3- or 4-element Array in
    # `@location` instead of a real `RBS::Location`:
    #
    #   [buffer, start_pos, end_pos]                   # no children
    #   [buffer, start_pos, end_pos, children_flat]    # with children
    #
    # `children_flat` is a flat Array of 4-tuples (kind, name, start, end);
    # absent optional children are encoded as `[:optional_absent, name,
    # nil, nil]` so the realiser can `each_slice(4)` uniformly.
    #
    # The `location` reader prepended below detects the Array form on
    # first access, allocates the real `RBS::Location`, applies the
    # children, and overwrites `@location` so subsequent reads hit the
    # normal C-side getter via `super`.
    #
    # For the load + resolve + `add_source` path no caller ever reads
    # `.location`, so the Location object is never materialised at all
    # — saving ~60K `RBS::Location.new` allocations per materialise plus
    # the per-child `_add_*_child` C calls.
    module LazyLocation
      def location
        loc = @location
        return loc unless loc.is_a?(::Array)
        realized = ::RBS::Location.new(loc[0], loc[1], loc[2])
        if (children = loc[3])
          children.each_slice(4) do |row|
            kind, name, s, e = row
            case kind
            when :required
              realized._add_required_child(name, s, e)
            when :optional_present
              realized._add_optional_child(name, s, e)
            when :optional_absent
              realized._add_optional_no_child(name)
            end
          end
        end
        @location = realized
      end
    end

    # Every RBS class whose materialiser passes `location:` ends up
    # holding our Array spec, so each one needs the lazy reader. List
    # is hand-curated to mirror the materialiser's call sites in
    # `ext/librbs/src/materialize/`.
    LAZY_LOCATION_CLASSES = [
      ::RBS::MethodType,

      ::RBS::AST::Annotation,
      ::RBS::AST::Comment,
      ::RBS::AST::TypeParam,

      ::RBS::AST::Directives::Use,
      ::RBS::AST::Directives::Use::SingleClause,
      ::RBS::AST::Directives::Use::WildcardClause,
      ::RBS::AST::Directives::ResolveTypeNames,

      ::RBS::AST::Declarations::Class,
      ::RBS::AST::Declarations::Class::Super,
      ::RBS::AST::Declarations::Module,
      ::RBS::AST::Declarations::Module::Self,
      ::RBS::AST::Declarations::Interface,
      ::RBS::AST::Declarations::TypeAlias,
      ::RBS::AST::Declarations::Constant,
      ::RBS::AST::Declarations::Global,
      ::RBS::AST::Declarations::ClassAlias,
      ::RBS::AST::Declarations::ModuleAlias,

      ::RBS::AST::Members::MethodDefinition,
      ::RBS::AST::Members::MethodDefinition::Overload,
      ::RBS::AST::Members::AttrAccessor,
      ::RBS::AST::Members::AttrReader,
      ::RBS::AST::Members::AttrWriter,
      ::RBS::AST::Members::Include,
      ::RBS::AST::Members::Extend,
      ::RBS::AST::Members::Prepend,
      ::RBS::AST::Members::InstanceVariable,
      ::RBS::AST::Members::ClassInstanceVariable,
      ::RBS::AST::Members::ClassVariable,
      ::RBS::AST::Members::Public,
      ::RBS::AST::Members::Private,
      ::RBS::AST::Members::Alias,

      ::RBS::Types::Bases::Bool,
      ::RBS::Types::Bases::Void,
      ::RBS::Types::Bases::Nil,
      ::RBS::Types::Bases::Top,
      ::RBS::Types::Bases::Bottom,
      ::RBS::Types::Bases::Self,
      ::RBS::Types::Bases::Instance,
      ::RBS::Types::Bases::Class,
      ::RBS::Types::Bases::Any,
      ::RBS::Types::Variable,
      ::RBS::Types::ClassSingleton,
      ::RBS::Types::ClassInstance,
      ::RBS::Types::Interface,
      ::RBS::Types::Alias,
      ::RBS::Types::Tuple,
      ::RBS::Types::Record,
      ::RBS::Types::Optional,
      ::RBS::Types::Union,
      ::RBS::Types::Intersection,
      ::RBS::Types::Proc,
      ::RBS::Types::Block,
      ::RBS::Types::Function,
      ::RBS::Types::Function::Param,
      ::RBS::Types::UntypedFunction,
      ::RBS::Types::Literal,
    ].freeze

    LAZY_LOCATION_CLASSES.each { |klass| klass.prepend(LazyLocation) }
  end
end
