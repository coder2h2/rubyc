x = 15
if x > 20
  puts "big"
elsif x > 10
  puts "medium"
else
  puts "small"
end

puts "ok" if x == 15
puts "no" unless x == 15
puts x > 10 ? "yes" : "no"

i = 0
sum = 0
while i < 10
  i += 1
  next if i % 2 == 0
  break if i > 7
  sum += i
end
puts sum

for k in 1..4
  puts k * 10
end

case x
when 1..10
  puts "1-10"
when 11..20
  puts "11-20"
else
  puts "other"
end
