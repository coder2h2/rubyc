module Printable
  def describe
    "I am #{self.class}"
  end
end

class Animal
  include Printable
  attr_accessor :name

  def initialize(name)
    @name = name
  end

  def speak
    "#{@name} makes a sound"
  end
end

class Dog < Animal
  def initialize(name, breed)
    super(name)
    @breed = breed
  end

  def speak
    super + " (woof! #{@breed})"
  end

  def self.kingdom
    "Animalia"
  end
end

d = Dog.new("Rex", "Labrador")
puts d.name
d.name = "Buddy"
puts d.speak
puts d.describe
puts d.is_a?(Animal)
puts d.respond_to?(:speak)
puts Dog.kingdom
puts Math.sqrt(16).to_i
