# frozen_string_literal: true

RSpec.describe Librbs do
  it "has a version number" do
    expect(Librbs::VERSION).not_to be_nil
  end

  it "loads native extension" do
    expect(defined?(Librbs::Native)).to eq("constant")
  end
end
