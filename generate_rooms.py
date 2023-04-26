#!/bin/env python3

import csv
import json
import random

PARETO_ALPHA = 1.161 # 80/20 rule.  See also: https://en.wikipedia.org/wiki/Pareto_distribution#Relation_to_the_%22Pareto_principle%22

# First load the roster of users from users.csv
users = []
with open("users.csv", "r", encoding="utf-8") as csvfile:
    reader = csv.DictReader(csvfile)
    for row in reader:
        username = row["username"]
        print("Found user [%s]" % username)
        users.append(username)
num_users = len(users)

#num_rooms = int(len(users) / 2)
max_num_rooms = num_users
# Then generate a bunch of rooms with their sizes from a power law distribution
room_sizes = []
room_sizes_sum = 0.0
for i in range(max_num_rooms):
    s = round(random.paretovariate(PARETO_ALPHA))
    if s > num_users:
        s = num_users
    if s < 2:
        #s = 2
        continue
    print("s = %d" % s)
    room_sizes.append(s)
    room_sizes_sum += s
num_rooms = len(room_sizes)
avg = room_sizes_sum / num_rooms

print("###################################")
print(f"{num_rooms} Total rooms")
print(f"Max = {max(room_sizes)}")
print(f"Min = {min(room_sizes)}")
print(f"Avg = {avg}")
print("###################################")

# Now assign the users (randomly) to the slots in the rooms
room_members = {}
for i in range(num_rooms):
    room_name = f"Room {i}"
    num_members = room_sizes[i]
    room_members[room_name] = random.sample(users, num_members)

# Now we need to sort of invert the list
# We need a list of the rooms to be created by each user,
# with the list of other users who should be invited to each
rooms_for_users = {"creators": []}
for room_name, room_users in room_members.items():
    room_info = {
        "creator": room_users[0],
        "name": room_name,
        "users": room_users[1:]
    }
    rooms_for_users["creators"].append(room_info)

# Save the room assignments to a file
with open("rooms.json", "w", encoding="utf-8") as jsonfile:
    jsonfile.write(json.dumps(rooms_for_users))

# Analyze the set of room assignments from the users' point of view
assignments = {}
for room, members in room_members.items():
    for member in members:
        if member in assignments:
            assignments[member] += 1
        else:
            assignments[member] = 1
roomless = []
in_all_rooms = []
centurions = []
for user in users:
    user_num_rooms = assignments.get(user, 0)
    if user_num_rooms < 1:
        roomless.append(user)
    if user_num_rooms == num_rooms:
        in_all_rooms.append(user)
    if user_num_rooms > 99:
        centurions.append(user)

print(f"{len(roomless)} users in zero rooms")
print(f"{len(in_all_rooms)} users in all rooms")
print(f"{len(centurions)} users in > 100 rooms")

