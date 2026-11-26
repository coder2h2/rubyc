def fib(n)
  return n if n <= 1
  fib(n - 1) + fib(n - 2)
end

puts fib(10)

def greet(who, msg = "Hi", *extras)
  puts "#{msg}, #{who}! extras=#{extras.length}"
end

greet("Alice")
greet("Bob", "Hello", 1, 2, 3)

def apply_twice(val)
  yield(yield(val))
end

puts apply_twice(3) { |n| n * 2 }

# Closure mutating outer variable
counter = 0
5.times do |i|
  counter += i + 1
end
puts counter

# Lambda
mul = ->(a, b) { a * b }
puts mul.call(6, 7)

make_adder = lambda do |base|
  ->(x) { base + x }
end
add10 = make_adder.call(10)
puts add10.call(25)
