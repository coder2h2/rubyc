def greet(name)
  "Hello, #{name}!"
end

names = ["World", "Ruby", "x86-64"]
names.each do |n|
  puts greet(n)
end

sum = (1..5).to_a.sum
puts "Sum of 1..5 is #{sum}"
